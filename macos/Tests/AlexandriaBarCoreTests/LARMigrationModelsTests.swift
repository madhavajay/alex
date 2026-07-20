import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif
import Testing
@testable import AlexandriaBarCore

@Suite(.serialized) struct LARMigrationModelsTests {
    @Test func decodesCompactMigrationStatus() throws {
        let json = #"{"job_id":"legacy-v1","state":"running","discovered":55000,"pending":1200,"migrated":53000,"skipped":700,"failed":100,"bytes_read":10093173145,"unique_bytes":828375040,"deduplicated_bytes":9264798105,"last_error":"archive volume is offline","paused":false,"running":true}"#

        let status = try JSONDecoder().decode(LARMigrationStatus.self, from: Data(json.utf8))

        #expect(status.jobID == "legacy-v1")
        #expect(status.state == .running)
        #expect(status.discovered == 55_000)
        #expect(status.pending == 1_200)
        #expect(status.migrated == 53_000)
        #expect(status.skipped == 700)
        #expect(status.failed == 100)
        #expect(status.bytesRead == 10_093_173_145)
        #expect(status.uniqueBytes == 828_375_040)
        #expect(status.deduplicatedBytes == 9_264_798_105)
        #expect(status.lastError == "archive volume is offline")
        #expect(status.running)
        #expect(!status.paused)
    }

    @Test func decodesPersistedCounterAliasesAndDerivesFlags() throws {
        let json = #"{"state":"paused","discovered_count":77,"pending_count":7,"migrated_count":65,"skipped_count":3,"failed_count":2,"bytes_read":1000,"unique_bytes":250,"dedup_bytes":750}"#

        let status = try JSONDecoder().decode(LARMigrationStatus.self, from: Data(json.utf8))

        #expect(status.discovered == 77)
        #expect(status.pending == 7)
        #expect(status.migrated == 65)
        #expect(status.skipped == 3)
        #expect(status.failed == 2)
        #expect(status.deduplicatedBytes == 750)
        #expect(status.paused)
        #expect(!status.running)
        #expect(status.processed == 68)
        #expect(status.completionFraction == 68.0 / 77.0)
        #expect(status.deduplicationFraction == 0.75)
    }

    @Test func missingFieldsAndFutureStateRemainDecodable() throws {
        let status = try JSONDecoder().decode(
            LARMigrationStatus.self,
            from: Data(#"{"state":"repacking"}"#.utf8))

        #expect(status.state == .unknown("repacking"))
        #expect(status.state.displayName == "Repacking")
        #expect(status.discovered == 0)
        #expect(status.lastError == nil)
        #expect(!status.paused)
        #expect(!status.running)
    }

    @Test func daemonNoJobAndPendingStatesHaveStablePresentation() throws {
        let notStarted = try JSONDecoder().decode(
            LARMigrationStatus.self,
            from: Data(#"{"state":"not_started"}"#.utf8))
        let pending = try JSONDecoder().decode(
            LARMigrationStatus.self,
            from: Data(#"{"state":"pending","pending":12}"#.utf8))

        #expect(notStarted.state == .idle)
        #expect(notStarted.state.displayName == "Not started")
        #expect(pending.state == .pending)
        #expect(pending.state.displayName == "Queued")
    }

    @Test func clientUsesMigrationStatusAndActionEndpoints() async throws {
        var requests: [(String, String)] = []
        LARMigrationURLProtocol.handler = { request in
            requests.append((request.httpMethod ?? "", request.url?.path ?? ""))
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            let body = request.httpMethod == "GET"
                ? Data(#"{"state":"running","migrated":4,"running":true}"#.utf8)
                : Data(#"{}"#.utf8)
            return (response, body)
        }
        defer { LARMigrationURLProtocol.handler = nil }

        let client = makeClient()
        let status = try await client.larMigrationStatus()
        #expect(status?.migrated == 4)
        #expect(try await client.larMigrationPause())
        #expect(try await client.larMigrationResume())
        #expect(try await client.larMigrationVerify())
        #expect(requests.map(\.0) == ["GET", "POST", "POST", "POST"])
        #expect(requests.map(\.1) == [
            "/admin/lar/migration",
            "/admin/lar/migration/pause",
            "/admin/lar/migration/resume",
            "/admin/lar/migration/verify",
        ])
    }

    @Test func olderDaemon404IsReportedAsUnsupported() async throws {
        LARMigrationURLProtocol.handler = { request in
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 404, httpVersion: nil, headerFields: nil)!
            return (response, Data(#"{"error":"not found"}"#.utf8))
        }
        defer { LARMigrationURLProtocol.handler = nil }

        let client = makeClient()
        #expect(try await client.larMigrationStatus() == nil)
        #expect(try await client.larMigrationPause() == false)
        #expect(try await client.larMigrationResume() == false)
        #expect(try await client.larMigrationVerify() == false)
    }

    @Test func urlSessionCancellationIsNormalized() async {
        LARMigrationURLProtocol.handler = { _ in throw URLError(.cancelled) }
        defer { LARMigrationURLProtocol.handler = nil }

        do {
            _ = try await makeClient().health()
            Issue.record("expected cancellation")
        } catch is CancellationError {
            // Intentional navigation cancellation must not be reported as a
            // daemon/network failure by higher-level browser state.
        } catch {
            Issue.record("expected CancellationError, got \(error)")
        }
    }

    private func makeClient() -> AlexandriaClient {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [LARMigrationURLProtocol.self]
        return AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "test-key"),
            session: URLSession(configuration: configuration))
    }
}

private final class LARMigrationURLProtocol: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var handler: ((URLRequest) throws -> (HTTPURLResponse, Data))?

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let handler = Self.handler else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
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
