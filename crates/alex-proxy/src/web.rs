use axum::extract::Path;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

const INDEX: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const STYLES: &str = include_str!("../web/styles.css");

fn asset(content_type: &'static str, body: &'static str) -> Response {
    let mut response = (StatusCode::OK, body).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

pub async fn index() -> Response {
    let mut response = asset("text/html; charset=utf-8", INDEX);
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    );
    response
}

pub async fn app_js() -> Response {
    asset("text/javascript; charset=utf-8", APP_JS)
}

pub async fn styles() -> Response {
    asset("text/css; charset=utf-8", STYLES)
}

pub async fn static_asset(Path(file): Path<String>) -> Response {
    let asset: Option<(&'static str, &'static [u8])> = match file.as_str() {
        "alex-icon.png" => Some(("image/png", include_bytes!("../web/assets/alex-icon.png"))),
        "onboarding-header.jpg" => Some((
            "image/jpeg",
            include_bytes!("../web/assets/onboarding-header.jpg"),
        )),
        "claude-code.png" => Some((
            "image/png",
            include_bytes!("../web/assets/logos/claude-code.png"),
        )),
        "codex.png" => Some(("image/png", include_bytes!("../web/assets/logos/codex.png"))),
        "gemini-cli.png" => Some((
            "image/png",
            include_bytes!("../web/assets/logos/gemini-cli.png"),
        )),
        "kimi-code.png" => Some((
            "image/png",
            include_bytes!("../web/assets/logos/kimi-code.png"),
        )),
        "grok-build.png" => Some((
            "image/png",
            include_bytes!("../web/assets/logos/grok-build.png"),
        )),
        "amp-code.svg" => Some((
            "image/svg+xml",
            include_bytes!("../web/assets/logos/amp-code.svg"),
        )),
        "openrouter.png" => Some((
            "image/png",
            include_bytes!("../web/assets/logos/openrouter.png"),
        )),
        "exo.png" => Some(("image/png", include_bytes!("../web/assets/logos/exo.png"))),
        "cursor-cli.png" => Some((
            "image/png",
            include_bytes!("../web/assets/logos/cursor-cli.png"),
        )),
        "pi.svg" => Some((
            "image/svg+xml",
            include_bytes!("../web/assets/logos/pi.svg"),
        )),
        "droid-cli.svg" => Some((
            "image/svg+xml",
            include_bytes!("../web/assets/logos/droid-cli.svg"),
        )),
        "opencode.png" => Some((
            "image/png",
            include_bytes!("../web/assets/logos/opencode.png"),
        )),
        "qwen-code.png" => Some((
            "image/png",
            include_bytes!("../web/assets/logos/qwen-code.png"),
        )),
        "goose.jpg" => Some((
            "image/jpeg",
            include_bytes!("../web/assets/logos/goose.jpg"),
        )),
        "oh-my-pi.png" => Some((
            "image/png",
            include_bytes!("../web/assets/logos/oh-my-pi.png"),
        )),
        "mini-swe-agent.png" => Some((
            "image/png",
            include_bytes!("../web/assets/logos/mini-swe-agent.png"),
        )),
        "jcode.png" => Some(("image/png", include_bytes!("../web/assets/logos/jcode.png"))),
        "hermes.png" => Some((
            "image/png",
            include_bytes!("../web/assets/logos/hermes.png"),
        )),
        "opensage-adk.png" => Some((
            "image/png",
            include_bytes!("../web/assets/logos/opensage-adk.png"),
        )),
        "pydantic-ai-harness.png" => Some((
            "image/png",
            include_bytes!("../web/assets/logos/pydantic-ai-harness.png"),
        )),
        _ => None,
    };
    let Some((content_type, body)) = asset else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mut response = (StatusCode::OK, body).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    fn assert_html_id(id: &str) {
        assert!(
            INDEX.contains(&format!("id=\"{id}\"")),
            "web shell is missing #{id}"
        );
    }

    fn javascript_function<'a>(source: &'a str, name: &str) -> &'a str {
        let async_declaration = format!("async function {name}(");
        let declaration = format!("function {name}(");
        let start = source
            .find(&async_declaration)
            .or_else(|| source.find(&declaration))
            .unwrap_or_else(|| panic!("client is missing function {name}"));
        let function = &source[start..];
        let end = function
            .match_indices('\n')
            .find_map(|(newline, _)| {
                let next_line = function[newline + 1..].lines().next()?.trim_start();
                (next_line.starts_with("async function ") || next_line.starts_with("function "))
                    .then_some(newline)
            })
            .unwrap_or(function.len());
        &function[..end]
    }

    fn assert_local_script_and_style_assets() {
        let lower = INDEX.to_ascii_lowercase();
        let inline_attribute = Regex::new(r#"(?i)\s(?:style|on[a-z]+)\s*="#).unwrap();
        assert!(
            !inline_attribute.is_match(INDEX),
            "CSP forbids inline style and event-handler attributes"
        );
        assert!(!lower.contains("<style"), "styles must stay in styles.css");
        assert!(!lower.contains("javascript:"));

        let script_start = lower.find("<script").expect("local app script tag");
        assert_eq!(lower.matches("<script").count(), 1);
        let script_tag_end = lower[script_start..].find('>').unwrap() + script_start;
        let script_tag = &lower[script_start..=script_tag_end];
        assert!(script_tag.contains("src=\"/ui/app.js\""));
        assert!(!script_tag.contains("http://"));
        assert!(!script_tag.contains("https://"));
        assert!(!script_tag.contains("//cdn"));
        let script_close =
            lower[script_tag_end + 1..].find("</script>").unwrap() + script_tag_end + 1;
        assert!(INDEX[script_tag_end + 1..script_close].trim().is_empty());

        for (link_start, _) in lower.match_indices("<link") {
            let rest = &lower[link_start..];
            let tag = &rest[..=rest.find('>').unwrap()];
            if tag.contains("rel=\"stylesheet\"") {
                assert!(tag.contains("href=\"/ui/styles.css\""));
                assert!(!tag.contains("http://"));
                assert!(!tag.contains("https://"));
                assert!(!tag.contains("//cdn"));
            }
        }
    }

    #[tokio::test]
    async fn web_entry_is_backend_free_and_hardened() {
        let response = index().await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "no-store, max-age=0"
        );
        let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(csp.contains("script-src 'self'"));
        assert!(csp.contains("style-src 'self'"));
        assert!(!csp.contains("unsafe-inline"));

        for id in [
            "auth-screen",
            "password-login",
            "password-login-form",
            "key-bootstrap",
            "remote-auth-form",
            "onboarding-view",
            "password-config-form",
            "web-password-form",
        ] {
            assert_html_id(id);
        }

        for destination in [
            "dashboard",
            "traces",
            "general",
            "updates",
            "providers",
            "harnesses",
            "credentials",
            "dario",
            "middleware",
            "notifications",
        ] {
            assert!(
                INDEX.contains(&format!("data-view=\"{destination}\"")),
                "navigation is missing {destination}"
            );
            assert_html_id(&format!("{destination}-view"));
        }

        for id in [
            "dashboard-stats",
            "dashboard-limits",
            "dashboard-accounts",
            "dashboard-harnesses",
            "dashboard-dario",
            "dashboard-traces",
            "update-versions",
            "update-status-detail",
            "update-confirm-dialog",
        ] {
            assert_html_id(id);
        }

        for id in [
            "provider-accounts",
            "provider-picker",
            "provider-tabs",
            "harness-list",
            "harness-tabs",
            "credential-inventory",
            "credential-ping-dialog",
            "credential-ping-summary",
            "credential-ping-rows",
            "dario-runtime",
            "dario-generations",
            "dario-caches",
            "middleware-rules",
            "middleware-activity",
            "notification-channels",
            "notification-log",
            "trace-browser",
            "trace-list",
            "trace-list-status",
            "trace-conversation",
            "trace-detail",
        ] {
            assert_html_id(id);
        }
        for class in [
            "trace-sessions",
            "trace-filters",
            "trace-list-labels",
            "trace-conversation",
            "trace-detail",
        ] {
            assert!(
                INDEX.contains(&format!("class=\"{class}")),
                "trace browser is missing .{class}"
            );
        }
        assert!(INDEX.contains("<select name=\"provider\""));
        assert!(INDEX.contains("<select name=\"harness\""));
        // The summaries handler's `status` filter is an exact integer match,
        // so the labels must not imply an unsupported status-class query.
        assert!(INDEX.contains("200 only"));
        assert!(INDEX.contains("400 only"));
        assert!(INDEX.contains("500 only"));
        assert!(INDEX.contains("Errors only"));
        assert_local_script_and_style_assets();
        assert!(INDEX.contains("src=\"/ui/assets/alex-icon.png\""));
        assert!(INDEX.contains("src=\"/ui/assets/onboarding-header.jpg\""));

        let external_asset =
            Regex::new(r#"(?i)(?:src\s*=\s*[\"']|url\(\s*[\"']?)(?:https?:)?//"#).unwrap();
        for (name, source) in [
            ("index.html", INDEX),
            ("app.js", APP_JS),
            ("styles.css", STYLES),
        ] {
            assert!(
                !external_asset.is_match(source),
                "{name} must not reference external assets"
            );
        }

        for endpoint in [
            "/web/auth/status",
            "/web/auth/login",
            "/web/auth/logout",
            "/admin/web/password",
            "/admin/web/onboarding",
        ] {
            assert!(APP_JS.contains(endpoint), "client must call {endpoint}");
        }

        // The new shell is a full control plane, not the previous four-tab
        // preview. Keep one representative read and mutation contract for
        // every destination pinned in the static asset test.
        for endpoint in [
            "/admin/analytics",
            "/admin/limits",
            "/admin/accounts/test",
            "/admin/accounts/analytics",
            "/admin/routing/",
            "/admin/auth/login/start",
            "/admin/auth/login/complete",
            "/admin/auth/reauth/start",
            "/admin/auth/reauth/submit",
            "/admin/auth/import-candidates",
            "/admin/auth/import",
            "/admin/openrouter/exposed",
            "/admin/exo/models",
            "/admin/auth/cliproxyapi",
            "/admin/harnesses/",
            "/admin/credentials",
            "/admin/run-keys",
            "/admin/dario/prompt-caches",
            "/admin/middleware/test",
            "/admin/protection",
            "/admin/fixtures",
            "/admin/sessions/",
            "/admin/notifications/validate",
            "/admin/notifications/discover-chat",
            "/admin/storage/prune",
            "/admin/update/channel",
        ] {
            assert!(APP_JS.contains(endpoint), "client must call {endpoint}");
        }
        assert!(APP_JS.contains("data-reauth-id"));
        assert!(APP_JS.contains("/reply.md"));
        assert!(APP_JS.contains("/traces/export.ndjson?"));
        assert!(APP_JS.contains("dry_run=true"));
        assert!(APP_JS.contains("/override"));
        assert!(APP_JS.contains("target=\"_blank\""));
        assert!(INDEX.contains("id=\"skip-onboarding\""));
        assert!(INDEX.contains("id=\"skip-onboarding-step\""));
        assert!(INDEX.contains("id=\"onboarding-error\""));
        assert_eq!(INDEX.matches("data-onboarding-step=").count(), 8);
        assert!(!INDEX.contains("<span class=\"nav-icon\">"));
        assert!(INDEX.contains("id=\"cliproxyapi-form\""));
        assert!(INDEX.contains("<dialog id=\"credential-ping-dialog\""));
        assert!(INDEX.contains("<dialog id=\"update-confirm-dialog\""));
        assert!(!INDEX.contains("credential-test-result"));
        assert_eq!(INDEX.matches("data-refresh-card").count(), 5);

        // Nested settings destinations stay deep-linkable; a bare #providers
        // hash lands on the first provider tab rather than an aggregate view.
        // Credential checks use one shared native modal from both dashboard
        // and provider entry points.
        assert!(APP_JS.contains("providerTab: PROVIDERS[0][0]"));
        assert!(APP_JS.contains("harnessTab: null"));
        assert!(APP_JS.contains(
            "\"pi\", \"claude\", \"codex\", \"grok\", \"amp\", \"gemini\", \"opencode\""
        ));
        assert!(APP_JS.contains("renderSectionTabs"));
        assert!(APP_JS.contains("encodeURIComponent(section)"));
        assert!(APP_JS.contains("not checked yet"));
        assert!(APP_JS.contains("#providers/claude"));
        assert!(STYLES.contains(".section-tabs{position:sticky"));
        assert!(STYLES.contains(".credential-ping-dialog::backdrop"));
        let open_ping = javascript_function(APP_JS, "openCredentialPingChecks");
        assert!(open_ping.contains("showModal"));
        let run_ping = javascript_function(APP_JS, "runCredentialPingChecks");
        assert!(run_ping.contains("/admin/accounts/test"));
        assert!(run_ping.contains("renderCredentialPingRows"));
        let refresh_updates = javascript_function(APP_JS, "refreshUpdateState");
        assert!(refresh_updates.contains("/admin/update"));
        assert!(refresh_updates.contains("/health"));
        let install_update = javascript_function(APP_JS, "installUpdate");
        assert!(install_update.contains("method: \"POST\""));
        assert!(install_update.contains("waitForUpdatedDaemon"));

        // The bootstrap key is page-memory only. Browser persistence is
        // limited to harmless presentation preferences.
        assert!(APP_JS.contains("adminKey: null"));
        assert!(!APP_JS.contains("localStorage.setItem(\"admin"));
        assert!(!APP_JS.contains("localStorage.setItem('admin"));
        assert!(APP_JS.contains("alex.web.refresh-seconds"));
        assert!(APP_JS.contains("alex.web.trace-columns"));

        // CSP permits no inline handlers; the client follows the same rule for
        // dynamically-created controls.
        let inline_handler_assignment =
            Regex::new(r"\.(?:onclick|onsubmit|onchange|ontoggle|onreset)\s*=").unwrap();
        assert!(!inline_handler_assignment.is_match(APP_JS));
        assert!(!APP_JS.to_ascii_lowercase().contains("onerror="));
        assert!(APP_JS.contains("addEventListener"));

        let page_size = Regex::new(r"const\s+TURN_PAGE_SIZE\s*=\s*20\s*;").unwrap();
        assert!(page_size.is_match(APP_JS));
        assert!(!APP_JS.contains("/transcript?limit=100"));
        let full_trace_fetch =
            Regex::new(r#"(?:api|apiText|fetch)\(\s*`/traces/\$\{[^}]+\}`\s*[,)]"#).unwrap();
        assert!(
            !full_trace_fetch.is_match(APP_JS),
            "the client must use body-free metadata rather than the full trace detail route"
        );

        // Fetch boundary: opening detail uses body-free metadata. The only
        // body/transcript request sites are the explicit details-toggle
        // loaders, so summary and metadata loading can never bulk-fetch them.
        let open_trace = javascript_function(APP_JS, "openTrace");
        assert!(open_trace.contains("/metadata"));
        assert!(!open_trace.contains("/body/"));
        assert!(!open_trace.contains("/transcript"));
        assert!(!open_trace.contains("/turn"));
        let load_traces = javascript_function(APP_JS, "loadTraces");
        assert!(load_traces.contains("/traces/summaries?"));
        assert!(load_traces.contains("/traces/sessions?limit=1000"));
        assert!(!load_traces.contains("/body/"));
        assert!(!load_traces.contains("/transcript"));
        assert!(!load_traces.contains("/metadata"));
        assert!(!load_traces.contains("/turn"));

        let render_detail = javascript_function(APP_JS, "renderTraceDetail");
        assert!(render_detail.contains("loadTraceBody"));
        assert!(render_detail.contains("addEventListener(\"toggle\""));
        assert!(!render_detail.contains("loadTranscript"));
        assert!(!render_detail.contains("/turn"));
        assert!(javascript_function(APP_JS, "loadTraceBody").contains("/body/"));
        assert!(javascript_function(APP_JS, "loadTranscript").contains("loadTranscriptPage"));

        // Selecting a summary composes two independent, bounded paths: the
        // metadata-only detail request and the middle-column conversation.
        // Keeping the transcript call out of openTrace makes that boundary
        // reviewable instead of hiding it behind metadata rendering.
        let select_trace = javascript_function(APP_JS, "selectTraceSummary");
        assert!(select_trace.contains("showTraceConversation"));
        assert!(select_trace.contains("openTrace"));
        let conversation = javascript_function(APP_JS, "showTraceConversation");
        assert!(conversation.contains("loadTranscript"));
        assert!(!conversation.contains("/body/"));
        assert!(!conversation.contains("/turn"));

        // Large-session contract: the transcript loader replaces one bounded
        // DOM page at a time, while opening one turn is the only path that
        // reaches the turn-body endpoint.
        let turn_loader = javascript_function(APP_JS, "loadTranscriptTurn");
        assert!(turn_loader.contains("/turn`"));
        assert_eq!(
            APP_JS.matches("/turn`").count(),
            1,
            "one explicit turn loader must remain the sole body-expansion path"
        );
        assert!(!turn_loader.contains("/body/"));
        assert!(!turn_loader.contains("/transcript/page"));
        let page_loader = javascript_function(APP_JS, "loadTranscriptPage");
        assert!(page_loader.contains("TURN_PAGE_SIZE"));
        assert!(page_loader.contains("/transcript/page?"));
        assert!(page_loader.contains("replaceTranscriptPage"));
        assert!(!page_loader.contains("/turn`"));
        assert!(!page_loader.contains("/body/"));
        assert!(javascript_function(APP_JS, "replaceTranscriptPage").contains("replaceChildren"));
    }

    #[tokio::test]
    async fn static_assets_are_allowlisted_and_immutable() {
        for (file, content_type) in [
            ("alex-icon.png", "image/png"),
            ("onboarding-header.jpg", "image/jpeg"),
            ("claude-code.png", "image/png"),
            ("codex.png", "image/png"),
            ("gemini-cli.png", "image/png"),
            ("kimi-code.png", "image/png"),
            ("grok-build.png", "image/png"),
            ("amp-code.svg", "image/svg+xml"),
            ("openrouter.png", "image/png"),
            ("exo.png", "image/png"),
            ("cursor-cli.png", "image/png"),
            ("pi.svg", "image/svg+xml"),
            ("droid-cli.svg", "image/svg+xml"),
            ("opencode.png", "image/png"),
            ("qwen-code.png", "image/png"),
            ("goose.jpg", "image/jpeg"),
            ("oh-my-pi.png", "image/png"),
            ("mini-swe-agent.png", "image/png"),
            ("jcode.png", "image/png"),
            ("hermes.png", "image/png"),
            ("opensage-adk.png", "image/png"),
            ("pydantic-ai-harness.png", "image/png"),
        ] {
            let response = static_asset(Path(file.into())).await;
            assert_eq!(response.status(), StatusCode::OK, "{file}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], content_type);
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "public, max-age=31536000, immutable"
            );
        }

        let missing = static_asset(Path("not-in-the-bundle.png".into())).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }
}
