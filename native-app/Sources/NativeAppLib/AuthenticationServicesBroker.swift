import Foundation

#if canImport(AuthenticationServices)
  import AuthenticationServices
  import AppKit
#endif

/// Stable broker error codes returned by the credential broker. These are
/// the codes the Rust CLI maps from when an external auth attempt fails.
/// See issue #13 / docs/SECURITY_POSTURE_AND_TESTING.md.
public enum BrokerErrorCode: String {
  case canceled
  case failed
  case invalidResponse
  case notHandled
  case unknown
  case unsupportedDomain
  case noCredentialSource
}

/// Result of a single credential request brokered through the OS.
public enum CredentialBrokerResult {
  case success(BrokerCredential)
  case denied
  case failure(BrokerErrorCode, String)
}

public struct BrokerCredential {
  public let domain: String
  public let url: String
  public let username: String
  public let password: String

  public init(domain: String, url: String, username: String, password: String) {
    self.domain = domain
    self.url = url
    self.username = username
    self.password = password
  }
}

/// Abstraction over the credential broker so `BrokerCore` can dispatch
/// without hard-binding to AuthenticationServices. Tests inject a stub
/// (see `BrokerCoreTests.swift`); production wires
/// `AppleAuthenticationServicesBroker`.
public protocol CredentialBroker {
  func login(url: String) -> CredentialBrokerResult
}

#if canImport(AuthenticationServices)

  /// Real broker that uses `ASAuthorizationController` + iCloud Keychain to
  /// surface credentials for an associated domain. The bridge is sync —
  /// the broker server thread blocks on a `DispatchSemaphore` while the
  /// run loop drives `ASAuthorizationController` on the main queue. See
  /// issue #13.
  ///
  /// TODO(#13): this scaffold compiles against the public AS API surface,
  /// but the end-to-end behavior (associated-domain matching, run-loop
  /// pumping inside an LSUIElement broker, ASAuthorizationError code
  /// stability across macOS versions) requires verification on a real
  /// macOS host with the app entitlements wired. See acceptance criteria
  /// in issue #13.
  public final class AppleAuthenticationServicesBroker: NSObject, CredentialBroker {

    private let presentationProvider: PresentationContextProvider

    public override init() {
      self.presentationProvider = PresentationContextProvider()
      super.init()
    }

    public func login(url rawURL: String) -> CredentialBrokerResult {
      guard let url = URL(string: rawURL),
        let host = url.host?.lowercased(),
        !host.isEmpty
      else {
        return .failure(.invalidResponse, "Invalid URL for native app login.")
      }

      let semaphore = DispatchSemaphore(value: 0)
      var captured: CredentialBrokerResult = .failure(.unknown, "broker did not complete")

      // ASAuthorizationController must be driven on the main thread. The
      // broker server runs accept() on a worker thread, so we hop to
      // main and block on a semaphore until the delegate fires.
      DispatchQueue.main.async {
        let request = ASAuthorizationPasswordProvider().createRequest()
        let controller = ASAuthorizationController(authorizationRequests: [request])

        let delegate = AuthorizationDelegate { result in
          captured = self.mapResult(result, host: host, url: rawURL)
          semaphore.signal()
        }
        controller.delegate = delegate
        controller.presentationContextProvider = self.presentationProvider

        // The broker keeps a strong reference to the delegate via the
        // controller until the request resolves; no manual retain is
        // needed beyond the closure.
        objc_setAssociatedObject(
          controller,
          &AppleAuthenticationServicesBroker.delegateKey,
          delegate,
          .OBJC_ASSOCIATION_RETAIN
        )

        controller.performRequests()
      }

      // Bound the sync wait so a hung credential picker cannot hold the
      // broker forever. `brokerRequestTimeoutMs` is the same constant
      // used for IPC. (issue #2)
      let timeout = DispatchTime.now() + .milliseconds(brokerRequestTimeoutMs)
      if semaphore.wait(timeout: timeout) == .timedOut {
        return .failure(.failed, "AuthenticationServices request timed out.")
      }
      return captured
    }

    private func mapResult(
      _ result: AuthorizationDelegate.Outcome,
      host: String,
      url: String
    ) -> CredentialBrokerResult {
      switch result {
      case .credential(let credential):
        return .success(
          BrokerCredential(
            domain: host,
            url: url,
            username: credential.user,
            password: credential.password
          ))
      case .canceled:
        return .denied
      case .error(let error):
        return .failure(brokerCode(for: error), error.localizedDescription)
      }
    }

    private func brokerCode(for error: Error) -> BrokerErrorCode {
      guard let asError = error as? ASAuthorizationError else {
        return .unknown
      }
      switch asError.code {
      case .canceled:
        return .canceled
      case .failed:
        return .failed
      case .invalidResponse:
        return .invalidResponse
      case .notHandled:
        return .notHandled
      case .unknown:
        return .unknown
      @unknown default:
        return .unknown
      }
    }

    private static var delegateKey: UInt8 = 0
  }

  /// Bridges the `ASAuthorizationController` callback into a single
  /// `Outcome` that maps cleanly onto `CredentialBrokerResult`.
  private final class AuthorizationDelegate: NSObject,
    ASAuthorizationControllerDelegate
  {
    enum Outcome {
      case credential(ASPasswordCredential)
      case canceled
      case error(Error)
    }

    private let completion: (Outcome) -> Void

    init(completion: @escaping (Outcome) -> Void) {
      self.completion = completion
    }

    func authorizationController(
      controller: ASAuthorizationController,
      didCompleteWithAuthorization authorization: ASAuthorization
    ) {
      if let credential = authorization.credential as? ASPasswordCredential {
        completion(.credential(credential))
      } else {
        completion(
          .error(
            NSError(
              domain: "dev.omt.apw",
              code: -1,
              userInfo: [
                NSLocalizedDescriptionKey: "Unexpected ASAuthorization credential type."
              ]
            )))
      }
    }

    func authorizationController(
      controller: ASAuthorizationController,
      didCompleteWithError error: Error
    ) {
      if let asError = error as? ASAuthorizationError, asError.code == .canceled {
        completion(.canceled)
      } else {
        completion(.error(error))
      }
    }
  }

  private final class PresentationContextProvider: NSObject,
    ASAuthorizationControllerPresentationContextProviding
  {
    func presentationAnchor(for controller: ASAuthorizationController)
      -> ASPresentationAnchor
    {
      // The broker is an LSUIElement app with no main window. The
      // ASAuthorizationController API still requires an anchor; falling
      // back to a borderless transient window keeps the call honest.
      // TODO(#13): verify this anchor lifetime against a notarized build.
      let window = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: 1, height: 1),
        styleMask: [.borderless],
        backing: .buffered,
        defer: false
      )
      return window
    }
  }

#endif
