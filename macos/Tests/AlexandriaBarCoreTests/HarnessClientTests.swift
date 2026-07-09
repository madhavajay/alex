import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite(.serialized) struct HarnessClientTests {
    @Test func harnesses404MapsToUnsupported() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/harnesses")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 404, httpVersion: nil, headerFields: nil)!
            return (response, Data(#"{"error":{"message":"not found"}}"#.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let session = URLSession(configuration: cfg)
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: session)

        let harnesses = try await client.harnesses()
        #expect(harnesses == nil)
    }

    @Test func daemonUpdateApplyPostsAndDecodesAccepted() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/update")
            #expect(request.httpMethod == "POST")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 202, httpVersion: nil, headerFields: nil)!
            return (response, Data(#"{"applying":true,"current":"0.1.0","latest":"0.2.0","update_available":true}"#.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let session = URLSession(configuration: cfg)
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: session)

        let response = try await client.daemonUpdateApply()
        #expect(response.applying)
        #expect(response.latest == "0.2.0")
    }

    @Test func daemonUpdateApply409UsesReason() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/update")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 409, httpVersion: nil, headerFields: nil)!
            return (response, Data(#"{"applying":false,"reason":"alex is managed by Homebrew - run `brew upgrade alex`"}"#.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let session = URLSession(configuration: cfg)
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: session)

        do {
            _ = try await client.daemonUpdateApply()
            Issue.record("daemonUpdateApply should reject managed installs")
        } catch AlexandriaClient.ClientError.daemonUpdateRejected(let reason) {
            #expect(reason == "alex is managed by Homebrew - run `brew upgrade alex`")
        }
    }

    @Test func refreshHarnessConfigPostsAndDecodes() async throws {
        let payload = #"""
        {"refreshed":true,"path":"/Users/x/.pi/agent/models.json","models_total":31,"added":["alex/claude-fable-5","alex/grok-4.5"],"removed":["alex/old-model"],"unchanged":29,"key":"reused","base_url":"http://127.0.0.1:4100"}
        """#
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/harnesses/pi/refresh-config")
            #expect(request.httpMethod == "POST")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let session = URLSession(configuration: cfg)
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: session)

        let response = try await client.refreshHarnessConfig("pi")
        #expect(response.refreshed == true)
        #expect(response.modelsTotal == 31)
        #expect(response.path.hasSuffix("models.json"))
        #expect(response.added == ["alex/claude-fable-5", "alex/grok-4.5"])
        #expect(response.removed == ["alex/old-model"])
        #expect(response.unchanged == 29)
        #expect(response.key == "reused")
        #expect(response.baseUrl == "http://127.0.0.1:4100")
    }

    @Test func connectHarnessPostsAndDecodesRichSummary() async throws {
        let payload = #"""
        {"path":"/Users/x/.pi/agent/models.json","models_total":28,"added":["alex/claude-opus-4-8"],"removed":[],"unchanged":0,"key":"minted","base_url":"http://127.0.0.1:4100","key_id":"rk-deadbeef"}
        """#
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/harnesses/pi/connect")
            #expect(request.httpMethod == "POST")
            #expect(request.url?.query?.contains("dry_run") != true)
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let session = URLSession(configuration: cfg)
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: session)

        let response = try await client.connectHarness("pi")
        #expect(response.refreshed == nil)
        #expect(response.modelsTotal == 28)
        #expect(response.key == "minted")
        #expect(response.keyId == "rk-deadbeef")
        #expect(response.added == ["alex/claude-opus-4-8"])
        #expect(response.removed.isEmpty)
    }

    @Test func connectHarnessPlanPostsDryRunAndDecodes() async throws {
        let payload = #"""
        {"plan":[
          {"path":"/Users/x/.pi/agent/models.json","action":"create","detail":"add provider 'alexandria' with 28 models"},
          {"path":"run-keys","action":"create","detail":"mint harness key"}
        ]}
        """#
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/harnesses/pi/connect")
            #expect(request.httpMethod == "POST")
            #expect(request.url?.query?.contains("dry_run=true") == true)
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let session = URLSession(configuration: cfg)
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: session)

        let response = try await client.connectHarnessPlan("pi")
        #expect(response.plan.count == 2)
        #expect(response.plan[0].action == "create")
        #expect(response.plan[0].detail.contains("alexandria"))
        #expect(response.plan[1].detail == "mint harness key")
    }

    @Test func disconnectHarnessPlanAndResultDecode() async throws {
        var sawDryRun = false
        let planPayload = #"""
        {"plan":[
          {"path":"/Users/x/.pi/agent/models.json","action":"modify","detail":"remove provider block"},
          {"path":"rk-deadbeef","action":"delete","detail":"revoke harness key deadbeefcafebabe"}
        ]}
        """#
        let resultPayload = #"""
        {"path":"/Users/x/.pi/agent/models.json","models_total":0,"added":[],"removed":["alex/claude-opus-4-8"],"unchanged":0,"key":"revoked","base_url":"http://127.0.0.1:4100","revoked":1,"was_connected":true}
        """#
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/harnesses/pi/disconnect")
            #expect(request.httpMethod == "POST")
            let isDry = request.url?.query?.contains("dry_run=true") == true
            if isDry { sawDryRun = true }
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data((isDry ? planPayload : resultPayload).utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let session = URLSession(configuration: cfg)
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: session)

        let plan = try await client.disconnectHarnessPlan("pi")
        #expect(sawDryRun)
        #expect(plan.plan.count == 2)
        #expect(plan.plan[0].detail == "remove provider block")

        let result = try await client.disconnectHarness("pi")
        #expect(result.wasConnected)
        #expect(result.revoked == 1)
        #expect(result.key == "revoked")
        #expect(result.removed == ["alex/claude-opus-4-8"])
        #expect(result.path.hasSuffix("models.json"))
    }
}

private final class HarnessEndpointURLProtocol: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var handler: ((URLRequest) throws -> (HTTPURLResponse, Data))?

    override class func canInit(with request: URLRequest) -> Bool {
        true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        guard let handler = Self.handler else {
            client?.urlProtocol(self, didFailWithError: AlexandriaClient.ClientError.http(500, "missing handler"))
            return
        }
        do {
            let (response, data) = try handler(request)
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: data)
            client?.urlProtocolDidFinishLoading(self)
        } catch {
            client?.urlProtocol(self, didFailWithError: error)
        }
    }

    override func stopLoading() {}
}
