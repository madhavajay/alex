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
            "onboarding",
            "dashboard",
            "traces",
            "general",
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
        ] {
            assert_html_id(id);
        }

        for id in [
            "provider-accounts",
            "provider-picker",
            "harness-list",
            "credential-inventory",
            "dario-runtime",
            "dario-generations",
            "dario-caches",
            "middleware-rules",
            "middleware-activity",
            "notification-channels",
            "notification-log",
            "trace-list",
            "trace-detail",
        ] {
            assert_html_id(id);
        }
        assert_local_script_and_style_assets();

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
            "/admin/openrouter/exposed",
            "/admin/exo/models",
            "/admin/auth/cliproxyapi",
            "/admin/harnesses/",
            "/admin/credentials",
            "/admin/run-keys",
            "/admin/dario/prompt-caches",
            "/admin/middleware/test",
            "/admin/protection",
            "/admin/notifications/validate",
            "/admin/notifications/discover-chat",
            "/admin/storage/prune",
            "/admin/update/channel",
        ] {
            assert!(APP_JS.contains(endpoint), "client must call {endpoint}");
        }
        assert!(APP_JS.contains("data-reauth-id"));
        assert!(APP_JS.contains("dry_run=true"));
        assert!(INDEX.contains("id=\"cliproxyapi-form\""));
        assert_eq!(INDEX.matches("data-refresh-card").count(), 5);

        // The bootstrap key is page-memory only. The sole browser-persisted
        // preference is the harmless refresh cadence.
        assert!(APP_JS.contains("adminKey: null"));
        assert!(!APP_JS.contains("localStorage.setItem(\"admin"));
        assert!(!APP_JS.contains("localStorage.setItem('admin"));
        assert!(APP_JS.contains("alex.web.refresh-seconds"));

        // CSP permits no inline handlers; the client follows the same rule for
        // dynamically-created controls.
        let inline_handler_assignment =
            Regex::new(r"\.(?:onclick|onsubmit|onchange|ontoggle|onreset)\s*=").unwrap();
        assert!(!inline_handler_assignment.is_match(APP_JS));
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
        let load_traces = javascript_function(APP_JS, "loadTraces");
        assert!(load_traces.contains("/traces/summaries?"));
        assert!(!load_traces.contains("/body/"));
        assert!(!load_traces.contains("/transcript"));

        let render_detail = javascript_function(APP_JS, "renderTraceDetail");
        assert!(render_detail.contains("loadTraceBody"));
        assert!(render_detail.contains("loadTranscript"));
        assert!(javascript_function(APP_JS, "loadTraceBody").contains("/body/"));
        assert!(javascript_function(APP_JS, "loadTranscript").contains("loadTranscriptPage"));

        // Large-session contract: the transcript loader replaces one bounded
        // DOM page at a time, while opening one turn is the only path that
        // reaches the turn-body endpoint.
        let turn_loader = javascript_function(APP_JS, "loadTranscriptTurn");
        assert!(turn_loader.contains("/turn`"));
        let page_loader = javascript_function(APP_JS, "loadTranscriptPage");
        assert!(page_loader.contains("TURN_PAGE_SIZE"));
        assert!(page_loader.contains("/transcript/page?"));
        assert!(page_loader.contains("replaceTranscriptPage"));
        assert!(!page_loader.contains("/turn`"));
        assert!(javascript_function(APP_JS, "replaceTranscriptPage").contains("replaceChildren"));
    }
}
