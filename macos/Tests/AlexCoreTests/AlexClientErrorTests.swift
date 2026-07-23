import Foundation
import Testing
@testable import AlexCore

extension AlexClientTests {
@Suite struct Errors {
    @Test(arguments: [401, 404, 500])
    func httpFailuresMapStatusAndBody(status: Int) async throws {
        AlexClientURLProtocolStub.reset()
        defer { AlexClientURLProtocolStub.reset() }
        let responseBody = #"{"error":{"message":"daemon rejected request"}}"#
        AlexClientURLProtocolStub.handler = { _ in
            .json(responseBody, status: status)
        }

        do {
            _ = try await makeStubbedAlexClient().health()
            Issue.record("Expected HTTP \(status) to throw")
        } catch AlexClient.ClientError.http(let code, let body) {
            #expect(code == status)
            #expect(body == responseBody)
        } catch {
            Issue.record("Unexpected error: \(error)")
        }
    }

    @Test func oversizedHTTPBodyIsTruncatedToTwoHundredCharacters() async throws {
        AlexClientURLProtocolStub.reset()
        defer { AlexClientURLProtocolStub.reset() }
        let oversized = String(repeating: "0123456789", count: 30)
        AlexClientURLProtocolStub.handler = { _ in .text(oversized, status: 500) }

        do {
            _ = try await makeStubbedAlexClient().health()
            Issue.record("Expected oversized error response to throw")
        } catch AlexClient.ClientError.http(let code, let body) {
            #expect(code == 500)
            #expect(body.count == 200)
            #expect(body == String(oversized.prefix(200)))
            #expect(AlexClient.ClientError.http(code, body).localizedDescription == "HTTP 500: \(body)")
        }
    }

    @Test func urlSessionFailureMapsToTransportError() async throws {
        AlexClientURLProtocolStub.reset()
        defer { AlexClientURLProtocolStub.reset() }
        AlexClientURLProtocolStub.handler = { _ in throw URLError(.cannotConnectToHost) }

        do {
            _ = try await makeStubbedAlexClient().health()
            Issue.record("Expected transport failure to throw")
        } catch AlexClient.ClientError.transport(let message) {
            #expect(!message.isEmpty)
        } catch {
            Issue.record("Unexpected error: \(error)")
        }
    }

    @Test func dario404StillDecodesAsFeatureUnavailable() async throws {
        AlexClientURLProtocolStub.reset()
        defer { AlexClientURLProtocolStub.reset() }
        AlexClientURLProtocolStub.handler = { _ in
            .json(#"{"error":{"message":"dario mode is not enabled"}}"#, status: 404)
        }
        let client = makeStubbedAlexClient()

        #expect(try await client.dario() == nil)
        #expect(try await client.darioDetail() == nil)
    }

    @Test func traceBodyUsesSameHTTPPrefixAndTransportMapping() async throws {
        AlexClientURLProtocolStub.reset()
        defer { AlexClientURLProtocolStub.reset() }
        var requestCount = 0
        AlexClientURLProtocolStub.handler = { _ in
            defer { requestCount += 1 }
            if requestCount == 0 {
                return .text(String(repeating: "x", count: 250), status: 500)
            }
            throw URLError(.networkConnectionLost)
        }
        let client = makeStubbedAlexClient()

        do {
            _ = try await client.traceBody(id: "trace-1", kind: .response)
            Issue.record("Expected trace body HTTP failure")
        } catch AlexClient.ClientError.http(let code, let body) {
            #expect(code == 500)
            #expect(body.count == 200)
        }

        do {
            _ = try await client.toolBody(id: "tool-1", kind: "result")
            Issue.record("Expected tool body transport failure")
        } catch AlexClient.ClientError.transport(let message) {
            #expect(!message.isEmpty)
        }
    }
}
}
