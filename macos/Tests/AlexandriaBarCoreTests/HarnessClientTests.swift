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
