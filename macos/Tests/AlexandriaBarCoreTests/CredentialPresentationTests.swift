import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct CredentialPresentationTests {
    @Test(arguments: [
        (ConnectClientAPI.anthropicMessages, "ANTHROPIC_BASE_URL=http://127.0.0.1:4100", "ANTHROPIC_API_KEY=alxk-real"),
        (.openAIChat, "OPENAI_BASE_URL=http://127.0.0.1:4100/v1", "OPENAI_API_KEY=alxk-real"),
        (.openAIResponses, "OPENAI_BASE_URL=http://127.0.0.1:4100/v1", "OPENAI_API_KEY=alxk-real"),
        (.geminiGenerateContent, "GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:4100", "GEMINI_API_KEY=alxk-real"),
    ])
    func snippetUsesCanonicalEnvironment(
        api: ConnectClientAPI, baseLine: String, keyLine: String
    ) {
        let snippet = ConnectSnippetBuilder.build(
            api: api, baseURL: "http://127.0.0.1:4100/", key: "alxk-real")
        #expect(snippet.contains("export \(baseLine)"))
        #expect(snippet.contains("export \(keyLine)"))
        #expect(!snippet.contains("MODEL="))
    }

    @Test func eachPickerChoiceProducesDistinctProtocolGuidance() {
        let snippets = ConnectClientAPI.allCases.map {
            ConnectSnippetBuilder.build(api: $0, baseURL: "http://localhost:4100", key: "alxk")
        }
        #expect(Set(snippets).count == ConnectClientAPI.allCases.count)
    }

    @Test func snippetSubstitutesKeyAndOptionalModel() {
        let snippet = ConnectSnippetBuilder.build(
            api: .openAIResponses,
            baseURL: "http://localhost:9999",
            key: "alxk-fresh",
            label: "  desktop-session  ",
            model: "  gpt-5.2  ")
        #expect(snippet == """
            # OpenAI Responses — POST $OPENAI_BASE_URL/responses
            export OPENAI_BASE_URL=http://localhost:9999/v1
            export OPENAI_API_KEY=alxk-fresh
            # next key label: desktop-session
            export MODEL=gpt-5.2
            """)
    }

    @Test(arguments: [
        (ConnectClientAPI.anthropicMessages, "/v1/messages", "x-api-key: alxk-real", "\"max_tokens\":256"),
        (.openAIChat, "/v1/chat/completions", "Authorization: Bearer alxk-real", "\"messages\":"),
        (.openAIResponses, "/v1/responses", "Authorization: Bearer alxk-real", "\"input\":"),
        (.geminiGenerateContent, "/v1beta/models/demo-model:generateContent", "x-goog-api-key: alxk-real", "\"contents\":"),
    ])
    func curlUsesProtocolEndpointAuthBodyAndOptionalTags(
        api: ConnectClientAPI,
        endpoint: String,
        authHeader: String,
        bodyMarker: String
    ) {
        let curl = ConnectSnippetBuilder.build(
            format: .curl,
            api: api,
            baseURL: "http://127.0.0.1:4100/",
            key: "alxk-real",
            label: "desktop-session",
            model: "demo-model")

        #expect(curl.hasPrefix("curl -sS -X POST"))
        #expect(curl.contains("http://127.0.0.1:4100\(endpoint)"))
        #expect(curl.contains(authHeader))
        #expect(curl.contains(bodyMarker))
        #expect(curl.contains("x-session-id: desktop-session"))
        #expect(curl.contains("x-alexandria-task: demo-model"))
        #expect(curl.contains("--data"))
    }

    @Test func curlWithoutInputsUsesRunnableDefaultAndOmitsOptionalTags() {
        let curl = ConnectSnippetBuilder.build(
            format: .curl,
            api: .openAIChat,
            baseURL: "http://localhost:4100",
            key: "alxk-real")
        #expect(curl.contains("\"model\":\"alex/claude-haiku-4-5\""))
        #expect(!curl.contains("x-session-id"))
        #expect(!curl.contains("x-alexandria-task"))
    }

    @Test func harnessJoinSelectsNewestActiveMatchingKey() throws {
        let json = #"""
        [
          {"id":"old","key_fingerprint":"oldfingerprint","kind":"harness","label":"Codex","run_id":null,"tags":{"harness":"codex"},"created_ms":100,"expires_ms":null,"last_used_ms":null,"use_count":1,"revoked":false},
          {"id":"revoked","key_fingerprint":"revokedfingerprint","kind":"harness","label":"Codex","run_id":null,"tags":{"harness":"codex"},"created_ms":300,"expires_ms":null,"last_used_ms":null,"use_count":1,"revoked":true},
          {"id":"new","key_fingerprint":"newfingerprint123","kind":"harness","label":"Codex","run_id":null,"tags":{"harness":"CoDeX"},"created_ms":200,"expires_ms":null,"last_used_ms":null,"use_count":1,"revoked":false},
          {"id":"run","key_fingerprint":"ordinary","kind":"run","label":"Other","run_id":null,"tags":{"harness":"codex"},"created_ms":400,"expires_ms":null,"last_used_ms":null,"use_count":1,"revoked":false}
        ]
        """#
        let keys = try JSONDecoder().decode([CredentialRunKey].self, from: Data(json.utf8))
        #expect(keys.activeHarnessKey(named: "codex")?.id == "new")
        #expect(keys.activeHarnessKey(named: "codex")?.shortFingerprint == "newfingerp")
        #expect(keys.activeHarnessKey(named: "claude") == nil)
    }

    @Test func outboundPresentationAddsKindSourceExpiryAndPingState() throws {
        let credentialsJSON = #"""
        {"inbound":{"admin_key":{"present":true},"local_key":{"present":true},"run_keys":[]},"outbound":[{"kind":"oauth","id":"openai-main","provider":"openai","present":true,"active":true,"identity":"person@example.com","expires_at_ms":7200000,"source":"vault"}]}
        """#
        let accountsJSON = #"""
        {"accounts":[{"id":"openai-main","provider":"openai","name":"main","kind":"oauth","paused":false,"status":"active","health":"healthy","needs_reauth":false,"expires_at_ms":7200000,"expires_in_s":7200}]}
        """#
        let healthJSON = #"""
        {"accounts":[{"id":"openai-main","provider":"openai","kind":"oauth","status":"active","token_expires_in_s":7200,"last_heartbeat":{"ok":false,"status":503,"latency_ms":10,"message":"down","ts_ms":10}}]}
        """#
        let credential = try JSONDecoder().decode(
            CredentialsResponse.self, from: Data(credentialsJSON.utf8)).outbound[0]
        let accounts = try JSONDecoder().decode(
            AccountsResponse.self, from: Data(accountsJSON.utf8)).accounts
        let health = try JSONDecoder().decode(
            HealthResponse.self, from: Data(healthJSON.utf8)).accounts
        let detail = credential.presentation(
            accounts: accounts, healthAccounts: health, now: Date(timeIntervalSince1970: 0))
        #expect(detail.kind == "OAuth subscription")
        #expect(detail.source == "vault")
        #expect(detail.expiry == "expires in 2h")
        #expect(detail.state == .needsReauth)
    }
}
