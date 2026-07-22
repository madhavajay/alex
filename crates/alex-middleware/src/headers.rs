use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

/// A canonical, credential-free header snapshot safe to expose to middleware.
///
/// Header names are lowercase. Deserialization and constructors discard secret,
/// authentication, cookie, and hop-by-hop fields rather than retaining redacted
/// values whose presence could itself expose routing details.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SafeHeaders(BTreeMap<String, Vec<String>>);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderSanitization {
    pub headers: SafeHeaders,
    pub removed_names: Vec<String>,
    pub invalid_names: Vec<String>,
    pub invalid_values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HeaderPatchError {
    #[error("invalid header name: {0}")]
    InvalidName(String),
    #[error("invalid header value for: {0}")]
    InvalidValue(String),
    #[error("middleware may not modify reserved header: {0}")]
    Reserved(String),
}

impl SafeHeaders {
    pub fn from_untrusted<I, N, V>(headers: I) -> HeaderSanitization
    where
        I: IntoIterator<Item = (N, V)>,
        N: Into<String>,
        V: IntoIterator,
        V::Item: Into<String>,
    {
        let mut result = HeaderSanitization::default();
        for (raw_name, raw_values) in headers {
            let raw_name = raw_name.into();
            let name = raw_name.to_ascii_lowercase();
            if !valid_header_name(&name) {
                result.invalid_names.push(raw_name);
                continue;
            }
            if is_secret_or_hop_header(&name) {
                result.removed_names.push(name);
                continue;
            }
            let values: Vec<String> = raw_values.into_iter().map(Into::into).collect();
            if values.iter().any(|value| !valid_header_value(value)) {
                result.invalid_values.push(name);
                continue;
            }
            result.headers.0.entry(name).or_default().extend(values);
        }
        result.removed_names.sort();
        result.removed_names.dedup();
        result
    }

    pub fn get(&self, name: &str) -> Option<&[String]> {
        self.0.get(&name.to_ascii_lowercase()).map(Vec::as_slice)
    }

    pub fn contains_key(&self, name: &str) -> bool {
        self.0.contains_key(&name.to_ascii_lowercase())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.0
            .iter()
            .map(|(name, values)| (name.as_str(), values.as_slice()))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn into_inner(self) -> BTreeMap<String, Vec<String>> {
        self.0
    }
}

impl<'de> Deserialize<'de> for SafeHeaders {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = BTreeMap::<String, Vec<String>>::deserialize(deserializer)?;
        Ok(Self::from_untrusted(raw).headers)
    }
}

impl fmt::Display for SafeHeaders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} safe headers", self.len())
    }
}

pub fn validate_header_patch(name: &str, value: Option<&str>) -> Result<(), HeaderPatchError> {
    let normalized = name.to_ascii_lowercase();
    if !valid_header_name(&normalized) {
        return Err(HeaderPatchError::InvalidName(name.to_owned()));
    }
    if is_reserved_patch_header(&normalized) {
        return Err(HeaderPatchError::Reserved(normalized));
    }
    if value.is_some_and(|value| !valid_header_value(value)) {
        return Err(HeaderPatchError::InvalidValue(normalized));
    }
    Ok(())
}

pub fn is_secret_or_hop_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "authorization"
            | "proxy-authorization"
            | "x-api-key"
            | "api-key"
            | "cookie"
            | "set-cookie"
            | "chatgpt-account-id"
            | "x-goog-api-key"
            | "x-alex-key"
            | "x-alex-api-key"
            | "x-alex-run-key"
            | "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    ) || name.ends_with("-api-key")
        || name.ends_with("-access-token")
        || name.ends_with("-auth-token")
        || name.ends_with("-security-token")
        || name.ends_with("-credential")
        || name.ends_with("-credentials")
}

pub fn is_reserved_patch_header(name: &str) -> bool {
    is_secret_or_hop_header(name)
        || matches!(
            name.to_ascii_lowercase().as_str(),
            "host"
                | "content-length"
                | "x-alex-no-substitute"
                | "x-alex-simulate-error"
                | "x-alex-run-id"
                | "x-alex-body-path"
        )
}

fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_header_value(value: &str) -> bool {
    !value.bytes().any(|byte| matches!(byte, b'\r' | b'\n' | 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitization_removes_secrets_and_canonicalizes_names() {
        let sanitized = SafeHeaders::from_untrusted([
            ("Authorization", vec!["Bearer secret"]),
            ("X-API-Key", vec!["local-secret"]),
            ("X-Provider-Access-Token", vec!["provider-secret"]),
            ("Content-Type", vec!["application/json"]),
            ("X-Request-ID", vec!["abc", "def"]),
        ]);
        assert_eq!(sanitized.headers.len(), 2);
        assert_eq!(
            sanitized.headers.get("content-type"),
            Some(["application/json".to_owned()].as_slice())
        );
        assert!(!format!("{sanitized:?}").contains("secret"));
        assert_eq!(sanitized.removed_names.len(), 3);
        assert!(sanitized
            .removed_names
            .contains(&"x-provider-access-token".to_owned()));
    }

    #[test]
    fn deserialization_cannot_smuggle_a_secret() {
        let headers: SafeHeaders = serde_json::from_value(serde_json::json!({
            "authorization": ["Bearer secret"],
            "accept": ["application/json"]
        }))
        .unwrap();
        assert!(!headers.contains_key("authorization"));
        assert!(headers.contains_key("accept"));
    }

    #[test]
    fn patches_reject_auth_hop_by_hop_and_control_headers() {
        for name in [
            "authorization",
            "cookie",
            "connection",
            "content-length",
            "x-alex-no-substitute",
        ] {
            assert!(matches!(
                validate_header_patch(name, None),
                Err(HeaderPatchError::Reserved(_))
            ));
        }
        assert!(validate_header_patch("x-client-label", Some("safe")).is_ok());
        assert!(matches!(
            validate_header_patch("x-client-label", Some("unsafe\r\nvalue")),
            Err(HeaderPatchError::InvalidValue(_))
        ));
    }
}
