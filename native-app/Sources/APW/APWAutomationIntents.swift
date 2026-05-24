#if canImport(AppIntents)
  import AppIntents
  import Foundation
  import NativeAppLib

  @available(macOS 13.0, *)
  struct APWLoginIntent: AppIntent {
    static var title: LocalizedStringResource = "Request APW Login"
    static var description = IntentDescription(
      "Requests a login credential through the APW broker. APW still requires user mediation before returning credential material."
    )
    static var openAppWhenRun = true

    @Parameter(title: "URL")
    var url: String

    init() {}

    init(url: String) {
      self.url = url
    }

    @MainActor
    func perform() async throws -> some IntentResult & ReturnsValue<String> {
      let data = try BrokerAutomation.performResponseData(
        operation: .login,
        url: url,
        requestId: "shortcuts-login"
      )
      return .result(value: String(decoding: data, as: UTF8.self))
    }
  }

  @available(macOS 13.0, *)
  struct APWFillIntent: AppIntent {
    static var title: LocalizedStringResource = "Request APW Fill"
    static var description = IntentDescription(
      "Requests a fill credential through the APW broker. APW still requires user mediation before returning credential material."
    )
    static var openAppWhenRun = true

    @Parameter(title: "URL")
    var url: String

    init() {}

    init(url: String) {
      self.url = url
    }

    @MainActor
    func perform() async throws -> some IntentResult & ReturnsValue<String> {
      let data = try BrokerAutomation.performResponseData(
        operation: .fill,
        url: url,
        requestId: "shortcuts-fill"
      )
      return .result(value: String(decoding: data, as: UTF8.self))
    }
  }

  @available(macOS 13.0, *)
  struct APWShortcutsProvider: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
      AppShortcut(
        intent: APWLoginIntent(),
        phrases: [
          "Request APW login with \(.applicationName)",
          "Get APW login credential with \(.applicationName)",
        ],
        shortTitle: "APW Login",
        systemImageName: "key.fill"
      )
      AppShortcut(
        intent: APWFillIntent(),
        phrases: [
          "Request APW fill with \(.applicationName)",
          "Fill with APW using \(.applicationName)",
        ],
        shortTitle: "APW Fill",
        systemImageName: "text.cursor"
      )
    }
  }
#endif
