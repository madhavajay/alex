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
        assert!(!csp.contains("unsafe-inline"));
        assert!(INDEX.contains("id=\"onboarding-view\""));
        assert!(INDEX.contains("id=\"middleware-view\""));
        assert!(INDEX.contains("id=\"traces-view\""));
        assert!(!INDEX.contains("https://cdn"));
        assert!(APP_JS.contains("/admin/auth/login/complete"));
        assert!(INDEX.contains("id=\"cliproxyapi-form\""));
        assert!(APP_JS.contains("/admin/auth/cliproxyapi"));
        assert!(APP_JS.contains("async function testCLIProxyAPI"));
        assert!(APP_JS.contains("x-alexandria-harness':'shared-web-onboarding'"));
        assert!(APP_JS.contains("/admin/middleware/test"));
        assert!(APP_JS.contains("method:'PUT'"));
        assert!(APP_JS.contains("/traces/summaries?"));
        assert!(APP_JS.contains("/metadata`"));
        assert!(APP_JS.contains("async function loadTraceBody"));
        assert!(APP_JS.contains("async function loadTranscript"));
        assert!(APP_JS.contains("record.explanation"));
        assert!(APP_JS.contains("Routing explanation"));
        assert!(APP_JS.contains("const TURN_PAGE_SIZE=20"));
        assert!(APP_JS.contains("/transcript/page?"));
        assert!(!APP_JS.contains("/transcript?limit=100"));

        // Fetch boundary: opening detail uses body-free metadata. The only
        // body/transcript request sites are the explicit details-toggle
        // loaders, so summary and metadata loading can never bulk-fetch them.
        let open_trace = APP_JS
            .split("async function openTrace")
            .nth(1)
            .unwrap()
            .split("async function loadTraceBody")
            .next()
            .unwrap();
        assert!(open_trace.contains("/metadata"));
        assert!(!open_trace.contains("/body/"));
        assert!(!open_trace.contains("/transcript"));
        let load_traces = APP_JS
            .split("async function loadTraces")
            .nth(1)
            .unwrap()
            .split("function facts")
            .next()
            .unwrap();
        assert!(load_traces.contains("/traces/summaries?"));
        assert!(!load_traces.contains("/body/"));
        assert!(!load_traces.contains("/transcript"));

        // Large-session contract: the transcript loader replaces one bounded
        // DOM page at a time, while opening one turn is the only path that
        // reaches the turn-body endpoint.
        let turn_loader = APP_JS
            .split("async function loadTranscriptTurn")
            .nth(1)
            .unwrap()
            .split("async function loadTranscriptPage")
            .next()
            .unwrap();
        assert!(turn_loader.contains("/turn`"));
        let page_loader = APP_JS
            .split("async function loadTranscriptPage")
            .nth(1)
            .unwrap()
            .split("async function loadTranscript(node)")
            .next()
            .unwrap();
        assert!(page_loader.contains("TURN_PAGE_SIZE"));
        assert!(page_loader.contains("/transcript/page?"));
        assert!(page_loader.contains("replaceTranscriptPage"));
        assert!(!page_loader.contains("/turn`"));
        assert!(APP_JS.contains("target.replaceChildren()"));
    }
}
