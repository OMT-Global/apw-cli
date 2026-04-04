import AppKit
import AuthenticationServices
import Darwin
import Foundation

private let runtimeDirectoryMode: mode_t = 0o700
private let statusFileMode: mode_t = 0o600
private let maxBrokerBytes = 32 * 1024
private let appSocketName = "broker.sock"
private let statusFileName = "status.json"
private let credentialsFileName = "credentials.json"
private let autoApproveEnv = "APW_NATIVE_APP_AUTO_APPROVE"

enum BrokerError: Error, CustomStringConvertible {
  case message(String)

  var description: String {
    switch self {
    case .message(let value):
      return value
    }
  }
}

struct CredentialsFile: Codable {
  struct Entry: Codable {
    let domain: String
    let url: String
    let username: String
    let password: String
  }

  var demo: Bool?
  let domains: [String]
  let credentials: [Entry]
}

struct RequestEnvelope: Codable {
  let requestId: String?
  let command: String
  let payload: [String: String]?
}

struct ResponseEnvelope: Codable {
  let ok: Bool
  let code: Int
  let payload: [String: AnyCodable]?
  let error: String?
  let requestId: String?
}

struct AppPaths {
  let runtimeRoot: URL
  let socketPath: URL
  let statusPath: URL
  let credentialsPath: URL

  static func resolve() -> AppPaths {
    let home = ProcessInfo.processInfo.environment["HOME"] ?? NSHomeDirectory()
    let root = URL(fileURLWithPath: home)
      .appendingPathComponent(".apw", isDirectory: true)
      .appendingPathComponent("native-app", isDirectory: true)
    return AppPaths(
      runtimeRoot: root,
      socketPath: root.appendingPathComponent(appSocketName),
      statusPath: root.appendingPathComponent(statusFileName),
      credentialsPath: root.appendingPathComponent(credentialsFileName)
    )
  }
}

struct AnyCodable: Codable {
  let value: Any

  init(_ value: Any) {
    self.value = value
  }

  init(from decoder: Decoder) throws {
    let container = try decoder.singleValueContainer()
    if let value = try? container.decode(Bool.self) {
      self.value = value
    } else if let value = try? container.decode(Int.self) {
      self.value = value
    } else if let value = try? container.decode(Double.self) {
      self.value = value
    } else if let value = try? container.decode(String.self) {
      self.value = value
    } else if let value = try? container.decode([String: AnyCodable].self) {
      self.value = value.mapValues(\.value)
    } else if let value = try? container.decode([AnyCodable].self) {
      self.value = value.map(\.value)
    } else {
      self.value = NSNull()
    }
  }

  func encode(to encoder: Encoder) throws {
    var container = encoder.singleValueContainer()
    switch value {
    case let value as Bool:
      try container.encode(value)
    case let value as Int:
      try container.encode(value)
    case let value as Double:
      try container.encode(value)
    case let value as String:
      try container.encode(value)
    case let value as [String: Any]:
      try container.encode(value.mapValues(AnyCodable.init))
    case let value as [Any]:
      try container.encode(value.map(AnyCodable.init))
    default:
      try container.encodeNil()
    }
  }
}

final class BrokerServer {
  private let paths: AppPaths
  private let startedAt = ISO8601DateFormatter().string(from: Date())

  init(paths: AppPaths) {
    self.paths = paths
  }

  func run() throws -> Never {
    try ensureRuntimeDirectory()
    try ensureCredentialsFile()
    try removeStaleSocket()
    try writeStatus(extra: [
      "serviceStatus": "starting"
    ])

    let descriptor = socket(AF_UNIX, SOCK_STREAM, 0)
    guard descriptor >= 0 else {
      throw BrokerError.message("Failed to create UNIX socket.")
    }

    var address = sockaddr_un()
    address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
    address.sun_family = sa_family_t(AF_UNIX)

    let socketPathBytes = Array(paths.socketPath.path.utf8)
    let maxLength = MemoryLayout.size(ofValue: address.sun_path)
    guard socketPathBytes.count + 1 < maxLength else {
      close(descriptor)
      throw BrokerError.message("Socket path is too long: \(paths.socketPath.path)")
    }

    withUnsafeMutableBytes(of: &address.sun_path) { rawBuffer in
      if let baseAddress = rawBuffer.baseAddress {
        memset(baseAddress, 0, rawBuffer.count)
      }
      rawBuffer.copyBytes(from: socketPathBytes + [0])
    }

    let bindResult = withUnsafePointer(to: &address) { pointer -> Int32 in
      pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { rebound in
        bind(descriptor, rebound, socklen_t(MemoryLayout<sockaddr_un>.size))
      }
    }
    guard bindResult == 0 else {
      let error = String(cString: strerror(errno))
      close(descriptor)
      throw BrokerError.message("Failed to bind broker socket: \(error)")
    }
    chmod(paths.socketPath.path, statusFileMode)

    guard listen(descriptor, 16) == 0 else {
      let error = String(cString: strerror(errno))
      close(descriptor)
      throw BrokerError.message("Failed to listen on broker socket: \(error)")
    }

    try writeStatus(extra: [
      "serviceStatus": "running",
      "pid": getpid(),
      "transport": "unix_socket",
    ])

    while true {
      let client = accept(descriptor, nil, nil)
      if client < 0 {
        continue
      }

      autoreleasepool {
        let handle = FileHandle(fileDescriptor: client, closeOnDealloc: true)
        do {
          let response = try handleRequest(from: handle)
          let data = try JSONEncoder().encode(response)
          try handle.write(contentsOf: data)
        } catch {
          let envelope = ResponseEnvelope(
            ok: false,
            code: 1,
            payload: nil,
            error: "Native app broker failure: \(error)",
            requestId: nil
          )
          if let data = try? JSONEncoder().encode(envelope) {
            try? handle.write(contentsOf: data)
          }
        }
        try? handle.close()
      }
    }
  }

  private func handleRequest(from handle: FileHandle) throws -> ResponseEnvelope {
    guard let data = try handle.readToEnd(), !data.isEmpty else {
      throw BrokerError.message("Empty broker request.")
    }
    guard data.count <= maxBrokerBytes else {
      throw BrokerError.message("Broker request payload too large.")
    }

    let request = try JSONDecoder().decode(RequestEnvelope.self, from: data)
    return try dispatch(request: request)
  }

  func dispatch(request: RequestEnvelope) throws -> ResponseEnvelope {
    switch request.command {
    case "status":
      return ResponseEnvelope(
        ok: true,
        code: 0,
        payload: statusPayload().mapValues(AnyCodable.init),
        error: nil,
        requestId: request.requestId
      )
    case "doctor":
      return ResponseEnvelope(
        ok: true,
        code: 0,
        payload: doctorPayload().mapValues(AnyCodable.init),
        error: nil,
        requestId: request.requestId
      )
    case "login":
      let url = request.payload?["url"] ?? ""
      return try loginResponse(for: url, requestId: request.requestId)
    default:
      return ResponseEnvelope(
        ok: false,
        code: 1,
        payload: nil,
        error: "Unsupported native app command: \(request.command)",
        requestId: request.requestId
      )
    }
  }

  private func statusPayload() -> [String: Any] {
    [
      "serviceStatus": "running",
      "startedAt": startedAt,
      "transport": "unix_socket",
      "bundleVersion": bundleVersion(),
      "socketPath": paths.socketPath.path,
      "supportedDomains": supportedDomains(),
      "authenticationServicesLinked": true,
    ]
  }

  func doctorPayload() -> [String: Any] {
    [
      "app": [
        "bundleVersion": bundleVersion(),
        "bundlePath": Bundle.main.bundleURL.path,
        "lsuiElement": true,
      ],
      "broker": statusPayload(),
      "credentialsPath": paths.credentialsPath.path,
      "guidance": [
        "Run `apw login https://example.com` to exercise the bootstrap credential flow.",
        "Set APW_NATIVE_APP_AUTO_APPROVE=1 to bypass the approval alert in non-interactive automation.",
      ],
    ]
  }

  private func loginResponse(for rawURL: String, requestId: String?) throws -> ResponseEnvelope {
    guard let url = URL(string: rawURL), let host = url.host?.lowercased(), !host.isEmpty else {
      return ResponseEnvelope(
        ok: false,
        code: 1,
        payload: nil,
        error: "Invalid URL for native app login.",
        requestId: requestId
      )
    }

    guard host == "example.com" else {
      return ResponseEnvelope(
        ok: false,
        code: 3,
        payload: nil,
        error: "The APW v2 bootstrap app currently supports only https://example.com.",
        requestId: requestId
      )
    }

    let credentials = try loadCredentials()
    guard let credential = credentials.credentials.first(where: { $0.domain == host }) else {
      return ResponseEnvelope(
        ok: false,
        code: 3,
        payload: nil,
        error: "No bootstrap credential is configured for \(host).",
        requestId: requestId
      )
    }

    let approved: Bool
    if ProcessInfo.processInfo.environment[autoApproveEnv] == "1" {
      approved = true
    } else {
      approved = promptForApproval(url: credential.url, username: credential.username)
    }

    if !approved {
      return ResponseEnvelope(
        ok: false,
        code: 1,
        payload: nil,
        error: "User denied the APW login request.",
        requestId: requestId
      )
    }

    return ResponseEnvelope(
      ok: true,
      code: 0,
      payload: [
        "status": AnyCodable("approved"),
        "url": AnyCodable(credential.url),
        "domain": AnyCodable(credential.domain),
        "username": AnyCodable(credential.username),
        "password": AnyCodable(credential.password),
        "transport": AnyCodable("unix_socket"),
        "userMediated": AnyCodable(true),
      ],
      error: nil,
      requestId: requestId
    )
  }

  private func promptForApproval(url: String, username: String) -> Bool {
    _ = ASCredentialIdentityStore.shared
    NSApplication.shared.setActivationPolicy(.accessory)
    let alert = NSAlert()
    alert.messageText = "Allow APW login?"
    alert.informativeText = "Return the bootstrap credential for \(url) as \(username)?"
    alert.alertStyle = .informational
    alert.addButton(withTitle: "Allow")
    alert.addButton(withTitle: "Deny")
    NSApp.activate(ignoringOtherApps: true)
    return alert.runModal() == .alertFirstButtonReturn
  }

  private func supportedDomains() -> [String] {
    (try? loadCredentials().domains) ?? ["example.com"]
  }

  private func loadCredentials() throws -> CredentialsFile {
    let data = try Data(contentsOf: paths.credentialsPath)
    return try JSONDecoder().decode(CredentialsFile.self, from: data)
  }

  private func ensureRuntimeDirectory() throws {
    try FileManager.default.createDirectory(
      at: paths.runtimeRoot,
      withIntermediateDirectories: true
    )
    chmod(paths.runtimeRoot.path, runtimeDirectoryMode)
  }

  private func ensureCredentialsFile() throws {
    guard !FileManager.default.fileExists(atPath: paths.credentialsPath.path) else {
      return
    }
    let content = CredentialsFile(
      demo: true,
      domains: ["example.com"],
      credentials: [
        .init(
          domain: "example.com",
          url: "https://example.com",
          username: "demo@example.com",
          password: "apw-demo-password"
        )
      ]
    )
    let data = try JSONEncoder().encode(content)
    try data.write(to: paths.credentialsPath, options: [.atomic])
    chmod(paths.credentialsPath.path, statusFileMode)
    fputs(
      "apw: info: created demo credentials file at \(paths.credentialsPath.path). "
        + "This file contains placeholder credentials — replace them with real entries before use.\n",
      stderr)
  }

  private func removeStaleSocket() throws {
    // Use unlink() directly rather than a check-then-remove pattern to avoid a TOCTOU
    // race where a symlink could be placed at the socket path between the existence
    // check and the removal. unlink() is atomic; ENOENT is not an error here.
    let result = unlink(paths.socketPath.path)
    if result != 0 && errno != ENOENT {
      throw NSError(
        domain: NSPOSIXErrorDomain, code: Int(errno),
        userInfo: [NSLocalizedDescriptionKey: "Failed to remove stale socket"])
    }
  }

  private func writeStatus(extra: [String: Any]) throws {
    var payload = statusPayload()
    for (key, value) in extra {
      payload[key] = value
    }
    let json = try JSONSerialization.data(withJSONObject: payload, options: [.prettyPrinted])
    try json.write(to: paths.statusPath, options: [.atomic])
    chmod(paths.statusPath.path, statusFileMode)
  }
}

func bundleVersion() -> String {
  if let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString")
    as? String,
    !version.isEmpty
  {
    return version
  }
  return "dev"
}

func main() -> Never {
  let paths = AppPaths.resolve()
  let command = CommandLine.arguments.dropFirst().first ?? "serve"
  let server = BrokerServer(paths: paths)

  do {
    switch command {
    case "serve":
      try server.run()
    case "doctor":
      let payload = server.doctorPayload()
      let data = try JSONSerialization.data(withJSONObject: payload, options: [.prettyPrinted])
      FileHandle.standardOutput.write(data)
      FileHandle.standardOutput.write(Data("\n".utf8))
      exit(0)
    case "request":
      guard CommandLine.arguments.count >= 3 else {
        throw BrokerError.message("Missing request command for `APW request`.")
      }
      let requestCommand = CommandLine.arguments[2]
      let requestPayload: [String: String]?
      if CommandLine.arguments.count >= 4 {
        requestPayload = try JSONDecoder().decode(
          [String: String].self,
          from: Data(CommandLine.arguments[3].utf8)
        )
      } else {
        requestPayload = nil
      }
      let response = try server.dispatch(
        request: RequestEnvelope(
          requestId: "oneshot",
          command: requestCommand,
          payload: requestPayload
        )
      )
      let data = try JSONEncoder().encode(response)
      FileHandle.standardOutput.write(data)
      FileHandle.standardOutput.write(Data("\n".utf8))
      exit(0)
    default:
      throw BrokerError.message("Unsupported APW app command: \(command)")
    }
  } catch {
    fputs("APW app failed: \(error)\n", stderr)
    exit(1)
  }
}

main()
