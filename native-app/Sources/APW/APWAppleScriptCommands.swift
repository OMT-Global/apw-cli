import AppKit
import Foundation
import NativeAppLib

enum APWAppleScriptCommandBridge {
  static func perform(
    operation: BrokerAutomationOperation,
    command: NSScriptCommand
  ) -> Any? {
    guard let url = command.directParameter as? String, !url.isEmpty else {
      command.scriptErrorNumber = 1
      command.scriptErrorString = "APW \(operation.rawValue) requires an HTTPS URL direct parameter."
      return nil
    }

    let request = {
      try BrokerAutomation.performResponseData(
        operation: operation,
        url: url,
        requestId: "applescript-\(operation.rawValue)"
      )
    }

    do {
      let data: Data
      if Thread.isMainThread {
        data = try performOffMainThreadWhilePumpingRunLoop(request)
      } else {
        data = try request()
      }
      return String(decoding: data, as: UTF8.self)
    } catch {
      command.scriptErrorNumber = 1
      command.scriptErrorString = "\(error)"
      return nil
    }
  }

  private static func performOffMainThreadWhilePumpingRunLoop(
    _ request: @escaping () throws -> Data
  ) throws -> Data {
    let result = AppleScriptCommandResultBox()

    DispatchQueue.global(qos: .userInitiated).async {
      result.set(Result(catching: request))
    }

    while result.value == nil {
      RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.01))
    }

    return try result.value!.get()
  }
}

private final class AppleScriptCommandResultBox {
  private let lock = NSLock()
  private var storage: Result<Data, Error>?

  var value: Result<Data, Error>? {
    lock.lock()
    defer { lock.unlock() }
    return storage
  }

  func set(_ value: Result<Data, Error>) {
    lock.lock()
    storage = value
    lock.unlock()
  }
}

@objc(APWRequestLoginCommand)
final class APWRequestLoginCommand: NSScriptCommand {
  override func performDefaultImplementation() -> Any? {
    APWAppleScriptCommandBridge.perform(operation: .login, command: self)
  }
}

@objc(APWRequestFillCommand)
final class APWRequestFillCommand: NSScriptCommand {
  override func performDefaultImplementation() -> Any? {
    APWAppleScriptCommandBridge.perform(operation: .fill, command: self)
  }
}
