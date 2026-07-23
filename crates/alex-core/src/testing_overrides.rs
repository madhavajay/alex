use std::net::IpAddr;

pub const FAKE_UPSTREAM_ENV: &str = "ALEX_FAKE_UPSTREAM";
pub const ALLOW_REMOTE_ENV: &str = "ALEX_TESTING_ALLOW_REMOTE";

pub fn resolve_endpoint_url(provider_env: &str, default: &str) -> String {
    resolve_endpoint_url_with(provider_env, default, |name| std::env::var(name).ok())
}

pub fn resolve_endpoint_override(provider_env: &str, default: &str) -> Option<String> {
    resolve_endpoint_override_with(provider_env, default, |name| std::env::var(name).ok())
}

pub fn allowed_override_url(value: &str) -> bool {
    let allow_remote = std::env::var(ALLOW_REMOTE_ENV).as_deref() == Ok("1");
    match parse_http_url(value.trim()) {
        Ok(parsed) if allow_remote || loopback_host(parsed.host) => true,
        Ok(parsed) => {
            tracing::error!(
                host = parsed.host,
                "ignoring non-loopback upstream override"
            );
            false
        }
        Err(error) => {
            tracing::error!(%error, "ignoring malformed upstream override");
            false
        }
    }
}

pub fn resolve_endpoint_url_with<F>(provider_env: &str, default: &str, lookup: F) -> String
where
    F: Fn(&str) -> Option<String>,
{
    resolve_endpoint_override_with(provider_env, default, lookup)
        .unwrap_or_else(|| default.to_string())
}

pub fn resolve_endpoint_override_with<F>(
    provider_env: &str,
    default: &str,
    lookup: F,
) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    let allow_remote = lookup(ALLOW_REMOTE_ENV).as_deref() == Some("1");
    for name in [provider_env, FAKE_UPSTREAM_ENV] {
        let Some(value) = lookup(name) else { continue };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match compose_override_url(value, default, allow_remote) {
            Ok(url) => return Some(url),
            Err(error) => tracing::error!(env = name, %error, "ignoring unsafe upstream override"),
        }
    }
    None
}

fn compose_override_url(base: &str, default: &str, allow_remote: bool) -> Result<String, String> {
    let parsed = parse_http_url(base)?;
    if !allow_remote && !loopback_host(parsed.host) {
        return Err(format!("override host '{}' is not loopback", parsed.host));
    }
    if parsed.suffix.contains('?') || parsed.suffix.contains('#') {
        return Err("override base must not contain a query or fragment".into());
    }
    let default = parse_http_url(default)?;
    let prefix = base.trim_end_matches('/');
    let suffix = if default.suffix.is_empty() {
        ""
    } else if default.suffix.starts_with('/') {
        default.suffix
    } else {
        return Err("default endpoint has an invalid path".into());
    };
    Ok(format!("{prefix}{suffix}"))
}

struct ParsedHttpUrl<'a> {
    host: &'a str,
    suffix: &'a str,
}

fn parse_http_url(value: &str) -> Result<ParsedHttpUrl<'_>, String> {
    let (scheme, rest) = value
        .split_once("://")
        .ok_or_else(|| "override must be an absolute http:// or https:// URL".to_string())?;
    if scheme != "http" && scheme != "https" {
        return Err("override must use http:// or https://".into());
    }
    let authority_end = rest
        .char_indices()
        .find_map(|(index, ch)| matches!(ch, '/' | '?' | '#').then_some(index))
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let suffix = &rest[authority_end..];
    if authority.is_empty() || authority.contains('@') {
        return Err("override must contain a host and no credentials".into());
    }
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        let end = bracketed
            .find(']')
            .ok_or_else(|| "override contains an invalid IPv6 host".to_string())?;
        let tail = &bracketed[end + 1..];
        if !tail.is_empty() && (!tail.starts_with(':') || tail[1..].parse::<u16>().is_err()) {
            return Err("override contains an invalid port".into());
        }
        let host = &bracketed[..end];
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err("override contains an invalid IPv6 host".into());
        }
        host
    } else {
        let (host, port) = authority
            .rsplit_once(':')
            .map_or((authority, None), |(host, port)| (host, Some(port)));
        if host.contains(':') {
            return Err("IPv6 override hosts must be bracketed".into());
        }
        if port.is_some_and(|port| port.parse::<u16>().is_err()) {
            return Err("override contains an invalid port".into());
        }
        host
    };
    if host.is_empty() {
        return Err("override must contain a host".into());
    }
    Ok(ParsedHttpUrl { host, suffix })
}

fn loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn resolve(values: &[(&str, &str)], provider_env: &str, default: &str) -> String {
        let values: HashMap<_, _> = values
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        resolve_endpoint_url_with(provider_env, default, |name| values.get(name).cloned())
    }

    #[test]
    fn provider_override_wins_and_preserves_endpoint_path() {
        let url = resolve(
            &[
                (FAKE_UPSTREAM_ENV, "http://127.0.0.1:4100/fake"),
                ("ALEX_UPSTREAM_XAI_URL", "http://localhost:4200/xai"),
            ],
            "ALEX_UPSTREAM_XAI_URL",
            "https://auth.x.ai/oauth2/token",
        );
        assert_eq!(url, "http://localhost:4200/xai/oauth2/token");
    }

    #[test]
    fn fake_upstream_fans_out_paths_and_queries() {
        let url = resolve(
            &[(FAKE_UPSTREAM_ENV, "http://[::1]:4300")],
            "ALEX_UPSTREAM_AMP_URL",
            "https://ampcode.com/api/internal?userDisplayBalanceInfo",
        );
        assert_eq!(url, "http://[::1]:4300/api/internal?userDisplayBalanceInfo");
    }

    #[test]
    fn remote_and_malformed_overrides_are_ignored() {
        for value in [
            "https://example.com",
            "http://192.168.1.20:8080",
            "file:///tmp/fake",
            "not-a-url",
            "http://[localhost]:8080",
            "http://localhost:99999",
        ] {
            assert_eq!(
                resolve(
                    &[("ALEX_UPSTREAM_OPENAI_URL", value)],
                    "ALEX_UPSTREAM_OPENAI_URL",
                    "https://api.openai.com/v1",
                ),
                "https://api.openai.com/v1"
            );
        }
    }

    #[test]
    fn loopback_range_and_remote_opt_in_are_allowed() {
        assert_eq!(
            resolve(
                &[("ALEX_UPSTREAM_OPENAI_URL", "http://127.42.0.9:4400")],
                "ALEX_UPSTREAM_OPENAI_URL",
                "https://api.openai.com/v1",
            ),
            "http://127.42.0.9:4400/v1"
        );
        assert_eq!(
            resolve(
                &[
                    ("ALEX_UPSTREAM_OPENAI_URL", "https://fake.example/root"),
                    (ALLOW_REMOTE_ENV, "1"),
                ],
                "ALEX_UPSTREAM_OPENAI_URL",
                "https://api.openai.com/v1",
            ),
            "https://fake.example/root/v1"
        );
    }

    #[test]
    fn rejected_provider_override_falls_back_to_fake_upstream() {
        assert_eq!(
            resolve(
                &[
                    ("ALEX_UPSTREAM_GEMINI_URL", "https://remote.example"),
                    (FAKE_UPSTREAM_ENV, "http://127.0.0.1:4500"),
                ],
                "ALEX_UPSTREAM_GEMINI_URL",
                "https://generativelanguage.googleapis.com/v1beta",
            ),
            "http://127.0.0.1:4500/v1beta"
        );
    }
}
