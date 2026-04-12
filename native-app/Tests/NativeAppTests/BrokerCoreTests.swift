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

    let denyServer = makeServer(root: root, decision: false)
    let denyResponse = try denyServer.dispatch(request: RequestEnvelope(
      requestId: "deny",
      command: "login",
      payload: ["url": "https://example.com"]
    ))
    XCTAssertEqual(denyResponse.ok, false)
    XCTAssertEqual(denyResponse.error, "User denied the APW login request.")
  }
}
