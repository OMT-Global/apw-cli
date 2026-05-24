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

    do {
      let data = try BrokerAutomation.performResponseData(
        operation: operation,
        url: url,
        requestId: "applescript-\(operation.rawValue)"
      )
      return String(decoding: data, as: UTF8.self)
    } catch {
      command.scriptErrorNumber = 1
      command.scriptErrorString = "\(error)"
      return nil
    }
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
