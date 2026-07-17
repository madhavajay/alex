//! Live harness matrix scaffolding. Each ignored test covers model routing,
//! subagent lineage in the transcript API, and tool bodies via executed_tools.

macro_rules! harness_matrix {
    ($install:ident, $lineage:ident, $tools:ident, $reason:literal) => {
        #[test]
        #[ignore = $reason]
        fn $install() {
            // install/configure alex/* and verify a traced round trip
            unimplemented!("TODO: live harness matrix model round trip");
        }

        #[test]
        #[ignore = $reason]
        fn $lineage() {
            // run a subagent; assert parent/child transcript lineage
            unimplemented!("TODO: live harness matrix subagent lineage");
        }

        #[test]
        #[ignore = $reason]
        fn $tools() {
            // execute a tool; assert tool_calls, executed_tools, and body endpoints
            unimplemented!("TODO: live harness matrix tool capture");
        }
    };
}

harness_matrix!(
    pi_install_and_model_roundtrip,
    pi_subagent_lineage_detected,
    pi_tool_capture_logged,
    "tools: fixture I3; lineage: fixture I9 + proxy unit tests"
);
harness_matrix!(
    claude_install_and_model_roundtrip,
    claude_subagent_lineage_detected,
    claude_tool_capture_logged,
    "connect/tools: proxy + harness_connect unit tests; live: fixture I2"
);
harness_matrix!(
    codex_install_and_model_roundtrip,
    codex_subagent_lineage_detected,
    codex_tool_capture_logged,
    "lineage: fixture I4; tools pending codex hook trust (see harness_connect docs)"
);
harness_matrix!(
    omp_install_and_model_roundtrip,
    omp_subagent_lineage_detected,
    omp_tool_capture_logged,
    "requires a live OMP installation"
);
harness_matrix!(
    opencode_install_and_model_roundtrip,
    opencode_subagent_lineage_detected,
    opencode_tool_capture_logged,
    "requires a live OpenCode installation"
);
harness_matrix!(
    mini_swe_agent_install_and_model_roundtrip,
    mini_swe_agent_subagent_lineage_detected,
    mini_swe_agent_tool_capture_logged,
    "requires a live mini-swe-agent installation"
);
harness_matrix!(
    kimi_install_and_model_roundtrip,
    kimi_subagent_lineage_detected,
    kimi_tool_capture_logged,
    "connect/config-rewrite: harness_connect unit tests (kimi_connection_*); live: `npm i -g @moonshot-ai/kimi-code`, `alex connect kimi`, run an alex/* model inside Kimi"
);

/// Kimi Code *subscription/provider* coverage (distinct from the harness/agent
/// matrix above). These exercise Alex acting as a Kimi client: import the
/// already-authed creds, proactively refresh the 15-minute token, route the
/// `kimi/*` models to `https://api.kimi.com/coding/v1`, and read usage.
///
/// Fast unit coverage runs in the normal suite:
///   - device-flow state machine: alex-auth `kimi_device_poll_state_machine`
///   - refresh decision:          alex-auth `kimi_refresh_decision_follows_expiry_margin`
///   - creds import (secs->ms):   alex-auth `kimi_import_builds_oauth_account_with_seconds_expiry`
///   - usage parsing:             alex-proxy `kimi_usage_payload_maps_windows_and_credits`
/// The live/networked legs below stay `#[ignore]` so `cargo test` is green offline.
mod kimi_subscription {
    #[test]
    #[ignore = "live: gated on ~/.kimi-code/credentials/kimi-code.json + KIMI_LIVE=1"]
    fn import_creds_then_refresh_before_expiry() {
        // `alex auth import kimi` adopts the CLI creds; a routed request after
        // >15min proves proactive refresh via refresh_token (no 401).
        unimplemented!("TODO: live Kimi provider — import creds, force expiry, assert auto-refresh");
    }

    #[test]
    #[ignore = "live: gated on a Kimi subscription + KIMI_LIVE=1"]
    fn route_kimi_model_chat_completion() {
        // POST /v1/chat/completions with model=kimi/k3 through Alex and assert a
        // non-empty completion from api.kimi.com/coding/v1.
        unimplemented!("TODO: live Kimi provider — route kimi/k3 chat/completions");
    }

    #[test]
    #[ignore = "live: gated on a Kimi subscription + KIMI_LIVE=1"]
    fn usage_probe_surfaces_quota() {
        // `alex status` / /admin/accounts shows Kimi quota windows fetched from
        // GET /coding/v1/usages.
        unimplemented!("TODO: live Kimi usage — assert quota windows recorded");
    }
}
harness_matrix!(
    gemini_install_and_model_roundtrip,
    gemini_subagent_lineage_detected,
    gemini_tool_capture_logged,
    "requires a live Gemini CLI installation"
);
harness_matrix!(
    qwen_install_and_model_roundtrip,
    qwen_subagent_lineage_detected,
    qwen_tool_capture_logged,
    "requires a live Qwen Code installation"
);
harness_matrix!(
    goose_install_and_model_roundtrip,
    goose_subagent_lineage_detected,
    goose_tool_capture_logged,
    "requires a live Goose installation"
);
harness_matrix!(
    opensage_install_and_model_roundtrip,
    opensage_subagent_lineage_detected,
    opensage_tool_capture_logged,
    "requires a live OpenSage ADK installation"
);
harness_matrix!(
    pydantic_ai_install_and_model_roundtrip,
    pydantic_ai_subagent_lineage_detected,
    pydantic_ai_tool_capture_logged,
    "requires a live Pydantic AI Harness installation"
);
harness_matrix!(
    stirrup_install_and_model_roundtrip,
    stirrup_subagent_lineage_detected,
    stirrup_tool_capture_logged,
    "requires a live Stirrup installation"
);
harness_matrix!(
    jcode_install_and_model_roundtrip,
    jcode_subagent_lineage_detected,
    jcode_tool_capture_logged,
    "requires a live JCode installation"
);
harness_matrix!(
    cursor_install_and_model_roundtrip,
    cursor_subagent_lineage_detected,
    cursor_tool_capture_logged,
    "requires a live Cursor CLI installation"
);
harness_matrix!(
    amp_install_and_model_roundtrip,
    amp_subagent_lineage_detected,
    amp_tool_capture_logged,
    "plugin unit tests; live gated on logged-in Amp CLI (fixture I5A)"
);
harness_matrix!(
    droid_install_and_model_roundtrip,
    droid_subagent_lineage_detected,
    droid_tool_capture_logged,
    "requires a live Droid CLI installation"
);
harness_matrix!(
    grok_install_and_model_roundtrip,
    grok_subagent_lineage_detected,
    grok_tool_capture_logged,
    "live testing is blocked (no Grok credits)"
);
harness_matrix!(
    hermes_install_and_model_roundtrip,
    hermes_subagent_lineage_detected,
    hermes_tool_capture_logged,
    "requires a live Hermes installation"
);
