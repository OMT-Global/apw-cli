import Foundation
import Darwin

private let defaultReconnectDelay: TimeInterval = 1.0

enum NativeHostError: Error, CustomStringConvertible {
    case message(String)

    var description: String {
        switch self {
        case let .message(value):
            return value
        }
    }
}

struct NativeHostConfig {
    let socketPath: String
    let helperPath: String
}

final class HelperSession {
    private let helperPath: String
    private var process: Process?
    private var stdinHandle: FileHandle?
    private var stdoutHandle: FileHandle?

    init(helperPath: String) {
        self.helperPath = helperPath
    }

    func invalidate() {
        if let process, process.isRunning {
            process.terminate()
        }
        process = nil
        stdinHandle = nil
        stdoutHandle = nil
    }

    func send(payload: Any) throws -> Any {
        try ensureRunning()
        guard let stdinHandle, let stdoutHandle else {
            throw NativeHostError.message("Helper process is not available.")
        }

        let data = try JSONSerialization.data(withJSONObject: payload, options: [])
        try writeFrame(data, to: stdinHandle)
        let response = try readFrame(from: stdoutHandle)
        return try JSONSerialization.jsonObject(with: response, options: [])
    }

    private func ensureRunning() throws {
        if let process, process.isRunning, stdinHandle != nil, stdoutHandle != nil {
            return
        }

        guard FileManager.default.isExecutableFile(atPath: helperPath) else {
            throw NativeHostError.message("Helper binary is not executable: \(helperPath)")
        }

        let input = Pipe()
        let output = Pipe()
        let process = Process()
        process.executableURL = URL(fileURLWithPath: helperPath)
        process.arguments = ["."]
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.standardError

        do {
            try process.run()
        } catch {
            invalidate()
            throw NativeHostError.message("Failed to start helper: \(error)")
        }

        self.process = process
        self.stdinHandle = input.fileHandleForWriting
        self.stdoutHandle = output.fileHandleForReading
    }
}

func parseArguments() throws -> NativeHostConfig {
    var socketPath: String?
    var helperPath: String?

    var index = 1
    while index < CommandLine.arguments.count {
        let argument = CommandLine.arguments[index]
        guard index + 1 < CommandLine.arguments.count else {
            throw NativeHostError.message("Missing value for argument \(argument)")
        }

        let value = CommandLine.arguments[index + 1]
        switch argument {
        case "--socket-path":
            socketPath = value
        case "--helper-path":
            helperPath = value
        default:
            throw NativeHostError.message("Unknown argument: \(argument)")
        }

        index += 2
    }

    guard let socketPath, !socketPath.isEmpty else {
        throw NativeHostError.message("Missing required --socket-path.")
    }
    guard let helperPath, !helperPath.isEmpty else {
        throw NativeHostError.message("Missing required --helper-path.")
    }

    return NativeHostConfig(socketPath: socketPath, helperPath: helperPath)
}

func readExact(from handle: FileHandle, count: Int) throws -> Data {
    var data = Data()
    while data.count < count {
        guard let chunk = try handle.read(upToCount: count - data.count), !chunk.isEmpty else {
            throw NativeHostError.message("Unexpected EOF.")
        }
        data.append(chunk)
    }
    return data
}

func readFrame(from handle: FileHandle) throws -> Data {
    let lengthData = try readExact(from: handle, count: 4)
    let length = lengthData.enumerated().reduce(UInt32(0)) { partial, item in
        partial | (UInt32(item.element) << (UInt32(item.offset) * 8))
    }
    let payloadLength = Int(UInt32(littleEndian: length))
    if payloadLength <= 0 || payloadLength > 16 * 1024 {
        throw NativeHostError.message("Invalid frame length: \(payloadLength)")
    }
    return try readExact(from: handle, count: payloadLength)
}

func writeFrame(_ payload: Data, to handle: FileHandle) throws {
    if payload.count > 16 * 1024 {
        throw NativeHostError.message("Outgoing payload exceeds max frame size.")
    }

    var length = UInt32(payload.count).littleEndian
    let header = withUnsafeBytes(of: &length) { Data($0) }
    try handle.write(contentsOf: header)
    try handle.write(contentsOf: payload)
}

func connectSocket(path: String) throws -> FileHandle {
    let descriptor = socket(AF_UNIX, Int32(SOCK_STREAM), 0)
    if descriptor < 0 {
        throw NativeHostError.message("Failed to create UNIX socket.")
    }

    var address = sockaddr_un()
    address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
    address.sun_family = sa_family_t(AF_UNIX)
    let pathBytes = Array(path.utf8)
    let maxLength = MemoryLayout.size(ofValue: address.sun_path)
    if pathBytes.count + 1 > maxLength {
        close(descriptor)
        throw NativeHostError.message("Socket path is too long: \(path)")
    }

    withUnsafeMutableBytes(of: &address.sun_path) { rawBuffer in
        if let baseAddress = rawBuffer.baseAddress {
            memset(baseAddress, 0, rawBuffer.count)
        }
        rawBuffer.copyBytes(from: pathBytes + [0])
    }

    let length = socklen_t(MemoryLayout<sockaddr_un>.size)
    let connectResult = withUnsafePointer(to: &address) { pointer -> Int32 in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { socketAddress in
            connect(descriptor, socketAddress, length)
        }
    }

    if connectResult != 0 {
        let error = String(cString: strerror(errno))
        close(descriptor)
        throw NativeHostError.message("Failed to connect to daemon socket: \(error)")
    }

    return FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
}

func bundleVersion() -> String {
    if let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
       !version.isEmpty
    {
        return version
    }
    return "dev"
}

func sendHello(to socket: FileHandle) throws {
    let hello: [String: Any] = [
        "type": "hello",
        "browser": "native",
        "version": bundleVersion(),
    ]
    let data = try JSONSerialization.data(withJSONObject: hello, options: [])
    try writeFrame(data, to: socket)
}

func sendErrorResponse(requestId: String, error: String, socket: FileHandle) {
    let response: [String: Any] = [
        "type": "response",
        "request_id": requestId,
        "ok": false,
        "error": error,
    ]

    guard let data = try? JSONSerialization.data(withJSONObject: response, options: []) else {
        return
    }
    try? writeFrame(data, to: socket)
}

func runConnection(config: NativeHostConfig, socket: FileHandle, helper: HelperSession) throws {
    try sendHello(to: socket)

    while true {
        let frame = try readFrame(from: socket)
        guard let message = try JSONSerialization.jsonObject(with: frame, options: []) as? [String: Any] else {
            throw NativeHostError.message("Daemon sent a malformed message.")
        }

        let type = message["type"] as? String ?? ""
        guard type == "request" else {
            continue
        }

        let requestId = (message["request_id"] as? String) ?? (message["requestId"] as? String) ?? ""
        guard let payload = message["payload"] else {
            sendErrorResponse(requestId: requestId, error: "Missing request payload.", socket: socket)
            continue
        }

        do {
            let helperResponse = try helper.send(payload: payload)
            let response: [String: Any] = [
                "type": "response",
                "request_id": requestId,
                "ok": true,
                "payload": helperResponse,
            ]
            let data = try JSONSerialization.data(withJSONObject: response, options: [])
            try writeFrame(data, to: socket)
        } catch {
            helper.invalidate()
            sendErrorResponse(
                requestId: requestId,
                error: "Native host helper failure: \(error)",
                socket: socket
            )
        }
    }
}

func main() -> Never {
    let config: NativeHostConfig
    do {
        config = try parseArguments()
    } catch {
        fputs("\(error)\n", stderr)
        exit(2)
    }

    let helper = HelperSession(helperPath: config.helperPath)

    while true {
        autoreleasepool {
            do {
                let socket = try connectSocket(path: config.socketPath)
                defer {
                    try? socket.close()
                }
                try runConnection(config: config, socket: socket, helper: helper)
            } catch {
                helper.invalidate()
                fputs("APW native host reconnecting after error: \(error)\n", stderr)
                Thread.sleep(forTimeInterval: defaultReconnectDelay)
            }
        }
    }
}

main()
