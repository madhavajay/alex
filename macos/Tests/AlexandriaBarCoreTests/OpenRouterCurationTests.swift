import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct OpenRouterCurationTests {
    func decode<T: Decodable>(_ json: String, as type: T.Type) throws -> T {
        try JSONDecoder().decode(T.self, from: Data(json.utf8))
    }

    @Test func availableExcludesExposedAndSortsCaseInsensitively() {
        let catalog = [
            "z-ai/glm-5.2",
            "Anthropic/Claude-Sonnet-5",
            "google/gemini-3.5-flash",
            "openai/gpt-4o",
        ]
        let exposed = ["z-ai/glm-5.2"]
        let available = OpenRouterCuration.available(catalog: catalog, exposed: exposed)
        // Exposed model is gone from the left column; the rest is alphabetical
        // regardless of case.
        #expect(
            available == [
                "Anthropic/Claude-Sonnet-5",
                "google/gemini-3.5-flash",
                "openai/gpt-4o",
            ])
    }

    @Test func availableFiltersBySearchCaseInsensitively() {
        let catalog = ["z-ai/glm-5.2", "google/gemini-3.5-flash", "openai/gpt-4o"]
        let available = OpenRouterCuration.available(
            catalog: catalog, exposed: [], search: "GEMINI")
        #expect(available == ["google/gemini-3.5-flash"])
    }

    @Test func availableDeduplicatesCatalog() {
        let available = OpenRouterCuration.available(
            catalog: ["a/one", "a/one", "b/two"], exposed: [])
        #expect(available == ["a/one", "b/two"])
    }

    @Test func addAndRemoveKeepExposedSortedAndUnique() {
        var exposed = ["z-ai/glm-5.2"]
        exposed = OpenRouterCuration.adding("anthropic/claude-sonnet-5", to: exposed)
        #expect(exposed == ["anthropic/claude-sonnet-5", "z-ai/glm-5.2"])
        // Adding a duplicate is a no-op.
        exposed = OpenRouterCuration.adding("z-ai/glm-5.2", to: exposed)
        #expect(exposed == ["anthropic/claude-sonnet-5", "z-ai/glm-5.2"])
        exposed = OpenRouterCuration.removing("anthropic/claude-sonnet-5", from: exposed)
        #expect(exposed == ["z-ai/glm-5.2"])
    }

    @Test func exposedResponseDecodesWithAndWithoutAvailable() throws {
        let both = try decode(
            #"{"exposed":["z-ai/glm-5.2"],"available":["z-ai/glm-5.2","openai/gpt-4o"]}"#,
            as: OpenRouterExposedResponse.self)
        #expect(both.exposed == ["z-ai/glm-5.2"])
        #expect(both.available == ["z-ai/glm-5.2", "openai/gpt-4o"])

        // The POST response omits `available`; it must still decode.
        let postOnly = try decode(
            #"{"exposed":["z-ai/glm-5.2"]}"#, as: OpenRouterExposedResponse.self)
        #expect(postOnly.exposed == ["z-ai/glm-5.2"])
        #expect(postOnly.available.isEmpty)
    }

    @Test func catalogResponseDecodes() throws {
        let catalog = try decode(
            #"{"models":["anthropic/claude-sonnet-5","z-ai/glm-5.2"]}"#,
            as: OpenRouterCatalogResponse.self)
        #expect(catalog.models == ["anthropic/claude-sonnet-5", "z-ai/glm-5.2"])
    }
}
