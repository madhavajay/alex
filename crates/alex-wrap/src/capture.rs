//! Capture sink for wrap sessions.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::catalog::WrapCapture;

/// One captured HTTP exchange (or upgrade attempt).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureEvent {
    pub seq: u64,
    pub kind: String,
    pub method: Option<String>,
    pub path: Option<String>,
    pub host: Option<String>,
    pub status: Option<u16>,
    pub body_len: Option<usize>,
    pub note: Option<String>,
}

/// In-memory + optional JSONL capture sink.
#[derive(Clone)]
pub struct CaptureLog {
    inner: Arc<Mutex<CaptureLogInner>>,
    policy: WrapCapture,
}

struct CaptureLogInner {
    events: VecDeque<CaptureEvent>,
    next_seq: u64,
    jsonl_path: Option<PathBuf>,
}

impl CaptureLog {
    pub fn new() -> Self {
        Self::with_policy(WrapCapture::default())
    }

    pub fn with_policy(policy: WrapCapture) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CaptureLogInner {
                events: VecDeque::new(),
                next_seq: 0,
                jsonl_path: None,
            })),
            policy,
        }
    }

    pub fn with_jsonl(path: impl Into<PathBuf>, policy: WrapCapture) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::File::create(&path)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(CaptureLogInner {
                events: VecDeque::new(),
                next_seq: 0,
                jsonl_path: Some(path),
            })),
            policy,
        })
    }

    pub fn policy(&self) -> &WrapCapture {
        &self.policy
    }

    pub fn should_record_path(&self, path: &str) -> bool {
        let path_only = path.split('?').next().unwrap_or(path);
        if self
            .policy
            .ignore_path_prefixes
            .iter()
            .any(|p| path_only.starts_with(p))
        {
            return false;
        }
        if self.policy.interesting_path_prefixes.is_empty() {
            return true;
        }
        self.policy
            .interesting_path_prefixes
            .iter()
            .any(|p| path_only.starts_with(p))
    }

    pub fn redact_path(&self, path: &str) -> String {
        let mut out = path.to_string();
        for key in &self.policy.redact_query_keys {
            out = redact_query_param(&out, key);
        }
        out
    }

    pub fn push(&self, mut event: CaptureEvent) {
        if let Some(ref path) = event.path {
            if !self.should_record_path(path) {
                return;
            }
            event.path = Some(self.redact_path(path));
        }
        let mut g = self.inner.lock().unwrap();
        g.next_seq += 1;
        event.seq = g.next_seq;
        if let Some(path) = &g.jsonl_path {
            if let Ok(line) = serde_json::to_string(&event) {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                    let _ = writeln!(f, "{line}");
                }
            }
        }
        g.events.push_back(event);
        let max = self.policy.max_events.max(1);
        while g.events.len() > max {
            g.events.pop_front();
        }
    }

    pub fn events(&self) -> Vec<CaptureEvent> {
        self.inner.lock().unwrap().events.iter().cloned().collect()
    }

    pub fn paths(&self) -> Vec<String> {
        self.events().into_iter().filter_map(|e| e.path).collect()
    }

    pub fn jsonl_path(&self) -> Option<PathBuf> {
        self.inner.lock().unwrap().jsonl_path.clone()
    }
}

impl Default for CaptureLog {
    fn default() -> Self {
        Self::new()
    }
}

fn redact_query_param(url: &str, key: &str) -> String {
    // path?a=1&key=SECRET&b=2 → path?a=1&key=<redacted>&b=2
    let Some((path, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let parts: Vec<String> = query
        .split('&')
        .map(|pair| {
            if let Some((k, _)) = pair.split_once('=') {
                if k.eq_ignore_ascii_case(key) {
                    return format!("{k}=<redacted>");
                }
            }
            pair.to_string()
        })
        .collect();
    format!("{path}?{}", parts.join("&"))
}

pub fn capture_dir_for(base: &Path, harness_id: &str) -> PathBuf {
    base.join("wrap").join(harness_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::WrapCapture;

    #[test]
    fn filters_and_redacts() {
        let policy = WrapCapture {
            interesting_path_prefixes: vec!["/api/".into()],
            ignore_path_prefixes: vec!["/api/static/".into()],
            redact_query_keys: vec!["rvt-token".into()],
            ..WrapCapture::default()
        };
        let log = CaptureLog::with_policy(policy);
        assert!(log.should_record_path("/api/internal"));
        assert!(!log.should_record_path("/api/static/x"));
        assert!(!log.should_record_path("/other"));
        assert_eq!(
            log.redact_path("/actors/ws?rvt-token=secret&x=1"),
            "/actors/ws?rvt-token=<redacted>&x=1"
        );
    }
}
