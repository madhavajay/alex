import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif
import Testing
@testable import AlexCore

/// A single serialized parent prevents the process-global URLProtocol handler
/// from being replaced by another client test while a request is in flight.
@Suite(.serialized) struct AlexClientTests {}

struct AlexClientRecordedRequest: @unchecked Sendable {
    let method: String
    let url: URL
    let headers: [String: String]
    let body: Data?

    var path: String { url.path }

    var query: [String: String] {
        let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []
        return Dictionary(uniqueKeysWithValues: items.compactMap { item in
            item.value.map { (item.name, $0) }
        })
    }

    func header(_ name: String) -> String? {
        headers.first { $0.key.caseInsensitiveCompare(name) == .orderedSame }?.value
    }

    func jsonBody() throws -> [String: Any] {
        let body = try #require(body)
        return try #require(JSONSerialization.jsonObject(with: body) as? [String: Any])
    }
}

struct AlexClientStubResponse: Sendable {
    let status: Int
    let headers: [String: String]
    let body: Data

    init(status: Int = 200, headers: [String: String] = [:], body: Data = Data()) {
        self.status = status
        self.headers = headers
        self.body = body
    }

    static func json(_ value: String, status: Int = 200, headers: [String: String] = [:]) -> Self {
        .init(status: status, headers: headers, body: Data(value.utf8))
    }

    static func text(_ value: String, status: Int = 200, headers: [String: String] = [:]) -> Self {
        .init(status: status, headers: headers, body: Data(value.utf8))
    }
}

final class AlexClientURLProtocolStub: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var handler: ((AlexClientRecordedRequest) throws -> AlexClientStubResponse)?
    private static let lock = NSLock()
    nonisolated(unsafe) private static var recorded: [AlexClientRecordedRequest] = []

    static func reset() {
        lock.lock()
        handler = nil
        recorded = []
        lock.unlock()
    }

    static var requests: [AlexClientRecordedRequest] {
        lock.lock()
        defer { lock.unlock() }
        return recorded
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        do {
            guard let url = request.url else {
                throw CocoaError(.fileReadUnknown)
            }
            let recordedRequest = AlexClientRecordedRequest(
                method: request.httpMethod ?? "GET",
                url: url,
                headers: request.allHTTPHeaderFields ?? [:],
                body: try Self.readBody(request))
            Self.lock.lock()
            Self.recorded.append(recordedRequest)
            let handler = Self.handler
            Self.lock.unlock()

            guard let handler else {
                throw CocoaError(.fileReadUnknown)
            }
            let response = try handler(recordedRequest)
            let http = HTTPURLResponse(
                url: recordedRequest.url,
                statusCode: response.status,
                httpVersion: nil,
                headerFields: response.headers)!
            client?.urlProtocol(self, didReceive: http, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: response.body)
            client?.urlProtocolDidFinishLoading(self)
        } catch {
            client?.urlProtocol(self, didFailWithError: error)
        }
    }

    override func stopLoading() {}

    private static func readBody(_ request: URLRequest) throws -> Data? {
        if let body = request.httpBody { return body }
        guard let stream = request.httpBodyStream else { return nil }
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 4_096)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            if count < 0 { throw stream.streamError ?? CocoaError(.fileReadUnknown) }
            if count == 0 { break }
            data.append(buffer, count: count)
        }
        return data
    }
}

func makeStubbedAlexClient() -> AlexClient {
    let configuration = URLSessionConfiguration.ephemeral
    configuration.protocolClasses = [AlexClientURLProtocolStub.self]
    return AlexClient(
        config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
        session: URLSession(configuration: configuration))
}

func expectStandardRequest(
    _ request: AlexClientRecordedRequest,
    method: String = "GET",
    path: String
) {
    #expect(request.method == method)
    #expect(request.path == path)
    #expect(request.header("x-api-key") == "local-test-key")
}
