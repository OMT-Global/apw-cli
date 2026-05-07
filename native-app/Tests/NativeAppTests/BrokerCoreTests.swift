import Darwin
import Foundation
import XCTest
@testable import NativeAppLib

private struct StubApprovalPrompter: ApprovalPrompter {
  let decision: Bool

  func prompt(url: String, username: String) -> Bool {
    decision
  }
}

final class BrokerCoreTests: XCTestCase {
  private func makePaths(_ root: URL) -> AppPaths {
    AppPaths(
      runtimeRoot: root,
      socketPath: root.appendingPathComponent("broker.sock"),
      statusPath: root.appendingPathComponent("status.json"),
      credentialsPath: root.appendingPathComponent("credentials.json")
    )
  }

  private func makeServer(
    root: URL,
    decision: Bool = true
  ) -> BrokerServer {
    BrokerServer(paths: makePaths(root), approvalPrompter: StubApprovalPrompter(decision: decision))
  }

  private func withDemoEnv(_ value: String?, run: () throws -> Void) rethrows {
    let previousValue = getenv("APW_DEMO").map { String(cString: $0) }
    if let value {
      setenv("APW_DEMO", value, 1)
    } else {
      unsetenv("APW_DEMO")
    }
    defer {
      if let previousValue {
        setenv("APW_DEMO", previousValue, 1)
      } else {
        unsetenv("APW_DEMO")
      }
    }
    try run()
  }

  private func writeCredentials(
    at path: URL,
    mode: Int = 0o600,
    contents: String = """
      {
        "domains": ["example.com"],
        "credentials": [
          {
            "domain": "example.com",
            "url": "https://example.com",
            "username": "demo@example.com",
            "password": "apw-demo-password"
          }
        ]
      }
      """
  ) throws {
    try FileManager.default.createDirectory(
      at: path.deletingLastPathComponent(),
      withIntermediateDirectories: true,
      attributes: nil
    )
    try contents.data(using: .utf8)!.write(to: path)
    try FileManager.default.setAttributes([.posixPermissions: mode], ofItemAtPath: path.path)
  }

  func testRequestEnvelopeDeserializationValidMalformedAndOversizedPayloads() throws {
    let root = URL(fileURLWithPath: NSTemporaryDirectory())
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    let server = makeServer(root: root)

    let valid = try JSONEncoder().encode(RequestEnvelope(
      requestId: "req-1",
      command: "status",
      payload: nil
    ))
    let response = try server.handleRequestData(valid)
    XCTAssertEqual(response.ok, true)
    XCTAssertEqual(response.requestId, "req-1")

    XCTAssertThrowsError(try server.handleRequestData(Data("{".utf8)))
    XCTAssertThrowsError(try server.handleRequestData(Data(repeating: 0x61, count: 32 * 1024 + 1)))
  }

  func testResponseEnvelopeSerializationCoversBrokerStatusCodes() throws {
    for code in [0, 1, 3] {
      let response = ResponseEnvelope(
        ok: code == 0,
        code: code,
        payload: ["code": AnyCodable(code)],
        error: code == 0 ? nil : "error-\(code)",
        requestId: "req-\(code)"
      )
      let data = try JSONEncoder().encode(response)
      let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
      XCTAssertEqual(json?["code"] as? Int, code)
      XCTAssertEqual(json?["requestId"] as? String, "req-\(code)")
    }
  }

  func testCredentialsParsingValidMalformedMissingAndWrongPermissions() throws {
    let root = URL(fileURLWithPath: NSTemporaryDirectory())
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    let server = makeServer(root: root)
    let credentialsPath = makePaths(root).credentialsPath

    try writeCredentials(at: credentialsPath)
    let credentials = try server.loadCredentials()
    XCTAssertEqual(credentials.credentials.first?.username, "demo@example.com")

    try writeCredentials(at: credentialsPath, contents: "{not-json")
    XCTAssertThrowsError(try server.loadCredentials())

    try? FileManager.default.removeItem(at: credentialsPath)
    XCTAssertThrowsError(try server.loadCredentials())

    try writeCredentials(at: credentialsPath, mode: 0o644)
    XCTAssertThrowsError(try server.loadCredentials()) { error in
      XCTAssertTrue(String(describing: error).contains("0600"))
    }
  }

  func testDemoCredentialsFileCreationRequiresDemoEnvironmentGate() throws {
    let defaultRoot = URL(fileURLWithPath: NSTemporaryDirectory())
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    let defaultPaths = makePaths(defaultRoot)
    try FileManager.default.createDirectory(
      at: defaultRoot,
      withIntermediateDirectories: true,
      attributes: nil
    )

    try withDemoEnv(nil) {
      try makeServer(root: defaultRoot).ensureCredentialsFile()
    }
    XCTAssertFalse(FileManager.default.fileExists(atPath: defaultPaths.credentialsPath.path))

    let demoRoot = URL(fileURLWithPath: NSTemporaryDirectory())
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    let demoPaths = makePaths(demoRoot)
    try FileManager.default.createDirectory(
      at: demoRoot,
      withIntermediateDirectories: true,
      attributes: nil
    )

    try withDemoEnv("1") {
      try makeServer(root: demoRoot).ensureCredentialsFile()
    }
    XCTAssertTrue(FileManager.default.fileExists(atPath: demoPaths.credentialsPath.path))
    let credentials = try makeServer(root: demoRoot).loadCredentials()
    XCTAssertEqual(credentials.demo, true)
    XCTAssertEqual(credentials.credentials.first?.username, "demo@example.com")
  }

  func testSocketListenerSetupAndTeardownUses0600Permissions() throws {
    let root = URL(fileURLWithPath: NSTemporaryDirectory())
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    let server = makeServer(root: root)
    try FileManager.default.createDirectory(
      at: root,
      withIntermediateDirectories: true,
      attributes: nil
    )

    let descriptor = try server.bindListeningSocket()
    defer { close(descriptor) }

    let attributes = try FileManager.default.attributesOfItem(atPath: makePaths(root).socketPath.path)
    let mode = (attributes[.posixPermissions] as? NSNumber)?.intValue
    XCTAssertEqual(mode, 0o600)

    try server.removeStaleSocket()
    XCTAssertFalse(FileManager.default.fileExists(atPath: makePaths(root).socketPath.path))
  }

  func testPromptForApprovalDecisionLogicUsesInjectedPrompter() throws {
    let root = URL(fileURLWithPath: NSTemporaryDirectory())
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    let paths = makePaths(root)
    try writeCredentials(at: paths.credentialsPath)

    let allowServer = makeServer(root: root, decision: true)
    let allowResponse = try allowServer.dispatch(request: RequestEnvelope(
      requestId: "allow",
      command: "login",
      payload: ["url": "https://example.com"]
    ))
    XCTAssertEqual(allowResponse.ok, true)
    XCTAssertEqual(allowResponse.payload?["intent"]?.value as? String, "login")

    let denyServer = makeServer(root: root, decision: false)
    let denyResponse = try denyServer.dispatch(request: RequestEnvelope(
      requestId: "deny",
      command: "login",
      payload: ["url": "https://example.com"]
    ))
    XCTAssertEqual(denyResponse.ok, false)
    XCTAssertEqual(denyResponse.error, "User denied the APW login request.")
  }

  func testFillDispatchUsesRealBrokerCredentialPathWithFillIntent() throws {
    let root = URL(fileURLWithPath: NSTemporaryDirectory())
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    let paths = makePaths(root)
    try writeCredentials(at: paths.credentialsPath)

    let response = try makeServer(root: root, decision: true).dispatch(request: RequestEnvelope(
      requestId: "fill-1",
      command: "fill",
      payload: ["url": "https://example.com"]
    ))

    XCTAssertEqual(response.ok, true)
    XCTAssertEqual(response.code, 0)
    XCTAssertEqual(response.requestId, "fill-1")
    XCTAssertEqual(response.payload?["status"]?.value as? String, "approved")
    XCTAssertEqual(response.payload?["intent"]?.value as? String, "fill")
    XCTAssertEqual(response.payload?["domain"]?.value as? String, "example.com")
    XCTAssertEqual(response.payload?["username"]?.value as? String, "demo@example.com")
    XCTAssertEqual(response.payload?["password"]?.value as? String, "apw-demo-password")
    XCTAssertEqual(response.payload?["transport"]?.value as? String, "unix_socket")
    XCTAssertEqual(response.payload?["userMediated"]?.value as? Bool, true)
  }

  func testCredentialRequestsRejectNonHttpsUrls() throws {
    let root = URL(fileURLWithPath: NSTemporaryDirectory())
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    let paths = makePaths(root)
    try writeCredentials(at: paths.credentialsPath)

    let server = makeServer(root: root, decision: true)
    for command in ["login", "fill"] {
      let response = try server.dispatch(request: RequestEnvelope(
        requestId: "\(command)-non-https",
        command: command,
        payload: ["url": "ftp://example.com"]
      ))
      XCTAssertEqual(response.ok, false, command)
      XCTAssertEqual(response.code, 1, command)
      XCTAssertEqual(response.error, "Native app credential requests require https URLs.", command)
    }
  }

  func testFillInvalidUnsupportedAndDeniedBehaviorMatchesLogin() throws {
    let root = URL(fileURLWithPath: NSTemporaryDirectory())
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    let paths = makePaths(root)
    try writeCredentials(at: paths.credentialsPath)

    for command in ["login", "fill"] {
      let invalid = try makeServer(root: root).dispatch(request: RequestEnvelope(
        requestId: "\(command)-invalid",
        command: command,
        payload: ["url": "not-a-url"]
      ))
      XCTAssertEqual(invalid.ok, false, command)
      XCTAssertEqual(invalid.code, 1, command)
      XCTAssertEqual(invalid.error, "Invalid URL for native app credential request.", command)

      let unsupported = try makeServer(root: root).dispatch(request: RequestEnvelope(
        requestId: "\(command)-unsupported",
        command: command,
        payload: ["url": "https://unsupported.example"]
      ))
      XCTAssertEqual(unsupported.ok, false, command)
      XCTAssertEqual(unsupported.code, 3, command)
      XCTAssertEqual(
        unsupported.error,
        "The APW v2 bootstrap app currently supports only https://example.com.",
        command
      )

      let denied = try makeServer(root: root, decision: false).dispatch(request: RequestEnvelope(
        requestId: "\(command)-denied",
        command: command,
        payload: ["url": "https://example.com"]
      ))
      XCTAssertEqual(denied.ok, false, command)
      XCTAssertEqual(denied.code, 1, command)
      XCTAssertEqual(denied.error, "User denied the APW login request.", command)
    }
  }
  func testDoctorPayloadDoesNotAdvertiseAmbientAutoApproveEscapeHatch() throws {
    let root = URL(fileURLWithPath: NSTemporaryDirectory())
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    let server = makeServer(root: root)

    let payload = server.doctorPayload()
    let guidance = payload["guidance"] as? [String]
    let broker = payload["broker"] as? [String: Any]

    XCTAssertNotNil(guidance)
    XCTAssertFalse(guidance?.contains(where: { $0.contains("APW_NATIVE_APP_AUTO_APPROVE") }) ?? true)
    XCTAssertEqual(broker?["requestTimeoutSeconds"] as? TimeInterval, 3)
  }
}
