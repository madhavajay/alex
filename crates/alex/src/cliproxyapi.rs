use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::Value;

const PROVIDER_NAME: &str = "alex";
const RUN_KEY_PREFIX: &str = "alxk-";

pub struct ReverseExportOptions<'a> {
    pub output: &'a Path,
    pub alex_api_base: &'a str,
    pub existing_key: Option<&'a str>,
    pub admin_base: Option<&'a str>,
    pub admin_key: Option<&'a str>,
    pub requested_models: &'a [String],
    pub cliproxyapi_version: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct ReverseExportResult {
    pub output: PathBuf,
    pub model_count: usize,
    pub key_id: Option<String>,
    pub schema: String,
    pub alex_api_base: String,
}

fn yaml_string(value: &str) -> String {
    serde_json::to_string(value).expect("JSON string serialization cannot fail")
}

fn validate_scoped_key(key: &str) -> Result<&str> {
    let key = key.trim();
    let suffix = key
        .strip_prefix(RUN_KEY_PREFIX)
        .context("CLIProxyAPI reverse config requires a scoped Alex run/harness key")?;
    if suffix.len() != 64 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("CLIProxyAPI reverse config requires a valid scoped Alex run/harness key");
    }
    Ok(key)
}

pub fn read_private_key_file(path: &Path) -> Result<String> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("reading scoped Alex key metadata {}", path.display()))?;
    if !metadata.is_file() {
        bail!(
            "scoped Alex key path is not a regular file: {}",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!(
                "scoped Alex key file must not be accessible by group or other users: {}",
                path.display()
            );
        }
    }
    let key = std::fs::read_to_string(path)
        .with_context(|| format!("reading scoped Alex key {}", path.display()))?;
    Ok(validate_scoped_key(&key)?.to_string())
}

pub fn normalize_alex_api_base(input: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(input.trim()).context("invalid Alex API URL")?;
    if !url.username().is_empty() || url.password().is_some() {
        bail!("Alex API URL must not contain credentials");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("Alex API URL must not contain a query or fragment");
    }
    let host = url.host_str().context("Alex API URL must include a host")?;
    if url.scheme() != "https" {
        let loopback = host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback());
        if url.scheme() != "http" || !loopback {
            bail!("Alex API URL must use HTTPS (loopback HTTP is allowed)");
        }
    }
    let path = url.path().trim_end_matches('/').to_string();
    if path.is_empty() {
        url.set_path("/v1");
    } else if path != "/v1" && !path.ends_with("/v1") {
        bail!("Alex API URL path must end in /v1");
    } else {
        url.set_path(&path);
    }
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn safe_reverse_model(id: &str) -> bool {
    let Some(model) = id.strip_prefix("alex/") else {
        return false;
    };
    if model.is_empty()
        || model.len() > 200
        || model.contains("..")
        || model.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return false;
    }
    let lower = model.to_ascii_lowercase();
    !["cliproxyapi/", "cliproxyapi:", "cliproxy/", "cliproxy:"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

pub fn select_reverse_models(catalog: &Value, requested: &[String]) -> Result<Vec<String>> {
    let available = catalog["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["id"].as_str())
        .filter(|id| safe_reverse_model(id))
        .map(String::from)
        .collect::<std::collections::BTreeSet<_>>();
    if available.is_empty() {
        bail!("Alex did not advertise any safe alex/* models");
    }
    if requested.is_empty() {
        return Ok(available.into_iter().collect());
    }

    let mut selected = std::collections::BTreeSet::new();
    for raw in requested {
        let id = if raw.starts_with("alex/") {
            raw.clone()
        } else {
            format!("alex/{raw}")
        };
        if !available.contains(&id) {
            bail!(
                "requested reverse model '{}' is not advertised by Alex",
                raw
            );
        }
        selected.insert(id);
    }
    Ok(selected.into_iter().collect())
}

fn parse_major(version: &str) -> Option<u64> {
    version
        .trim()
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse().ok())
}

fn validate_capabilities(payload: &Value, cliproxyapi_version: Option<&str>) -> Result<String> {
    let reverse = &payload["integrations"]["cliproxyapi_reverse"];
    let schema = reverse["schema"]
        .as_str()
        .context("Alex capability response is missing the CLIProxyAPI reverse schema")?;
    if schema != alex_proxy::CLIPROXYAPI_REVERSE_SCHEMA {
        bail!("Alex reported unsupported CLIProxyAPI reverse schema '{schema}'");
    }
    let minimum = reverse["minimum_cliproxyapi_major"]
        .as_u64()
        .context("Alex capability response is missing minimum_cliproxyapi_major")?;
    let advertised = reverse["capabilities"]
        .as_array()
        .context("Alex capability response is missing CLIProxyAPI reverse capabilities")?
        .iter()
        .filter_map(Value::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    for required in alex_proxy::CLIPROXYAPI_REVERSE_CAPABILITIES {
        if !advertised.contains(required) {
            bail!("Alex capability response is missing required capability '{required}'");
        }
    }
    if let Some(version) = cliproxyapi_version {
        let major = parse_major(version)
            .with_context(|| format!("could not parse CLIProxyAPI version '{version}'"))?;
        if major < minimum {
            bail!("CLIProxyAPI {version} is unsupported; Alex requires major {minimum} or newer");
        }
    }
    Ok(schema.to_string())
}

pub fn render_reverse_config(
    alex_api_base: &str,
    key: &str,
    models: &[String],
    cliproxyapi_version: Option<&str>,
) -> Result<String> {
    let key = validate_scoped_key(key)?;
    if models.is_empty() || models.iter().any(|model| !safe_reverse_model(model)) {
        bail!("reverse config requires safe alex/* models");
    }
    let harness_version = cliproxyapi_version.unwrap_or("v7-compatible");
    let capabilities = alex_proxy::CLIPROXYAPI_REVERSE_CAPABILITIES.join(",");
    let mut yaml = format!(
        "# Generated by Alex. This file contains a scoped harness credential.\n\
         # Keep it private and merge this fragment into CLIProxyAPI's config.yaml.\n\
         passthrough-headers: true\n\
         openai-compatibility:\n\
           - name: {provider}\n\
             prefix: {provider}\n\
             base-url: {base}\n\
             headers:\n\
               X-Alexandria-Harness: {harness}\n\
               X-Alexandria-Harness-Version: {harness_version}\n\
               X-Alexandria-Integration-Schema: {schema}\n\
               X-Alexandria-Capabilities: {capabilities}\n\
               X-Alexandria-Route-Chain: {route_chain}\n\
             api-key-entries:\n\
               - api-key: {key}\n\
             models:\n",
        provider = yaml_string(PROVIDER_NAME),
        base = yaml_string(alex_api_base),
        harness = yaml_string("cliproxyapi"),
        harness_version = yaml_string(harness_version),
        schema = yaml_string(alex_proxy::CLIPROXYAPI_REVERSE_SCHEMA),
        capabilities = yaml_string(&capabilities),
        route_chain = yaml_string("cliproxyapi"),
        key = yaml_string(key),
    );
    for model in models {
        let alias = model.trim_start_matches("alex/");
        yaml.push_str(&format!(
            "               - name: {}\n                 alias: {}\n                 input-modalities: [text]\n                 output-modalities: [text]\n",
            yaml_string(model),
            yaml_string(alias),
        ));
    }
    Ok(yaml)
}

fn ensure_output_available(path: &Path) -> Result<()> {
    if path.exists() {
        bail!("refusing to overwrite existing file {}", path.display());
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        if !parent.is_dir() {
            bail!("output directory does not exist: {}", parent.display());
        }
    }
    Ok(())
}

fn write_private(path: &Path, contents: &str) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("creating private CLIProxyAPI config {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("writing CLIProxyAPI config {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing CLIProxyAPI config {}", path.display()))?;
    Ok(())
}

async fn response_json(response: reqwest::Response, context: &str) -> Result<Value> {
    let status = response.status();
    let body = response.bytes().await.unwrap_or_default();
    if !status.is_success() {
        bail!(
            "{context} failed (HTTP {}): {}",
            status.as_u16(),
            String::from_utf8_lossy(&body)
        );
    }
    serde_json::from_slice(&body).with_context(|| format!("{context} returned invalid JSON"))
}

async fn mint_reverse_key(
    client: &reqwest::Client,
    admin_base: &str,
    admin_key: &str,
) -> Result<(String, String)> {
    let response = client
        .post(format!(
            "{}/admin/run-keys",
            admin_base.trim_end_matches('/')
        ))
        .header("x-api-key", admin_key)
        .json(&serde_json::json!({
            "kind": "harness",
            "label": "cliproxyapi",
            "tags": {"integration": "cliproxyapi-reverse"}
        }))
        .send()
        .await
        .context("contacting the Alex admin API to mint a CLIProxyAPI harness key")?;
    let body = response_json(response, "minting the CLIProxyAPI harness key").await?;
    let id = body["id"]
        .as_str()
        .context("Alex key response is missing id")?
        .to_string();
    let key = body["key"]
        .as_str()
        .context("Alex key response is missing key")?
        .to_string();
    Ok((id, key))
}

async fn revoke_reverse_key(
    client: &reqwest::Client,
    admin_base: &str,
    admin_key: &str,
    key_id: &str,
) {
    let _ = client
        .delete(format!(
            "{}/admin/run-keys/{key_id}",
            admin_base.trim_end_matches('/')
        ))
        .header("x-api-key", admin_key)
        .send()
        .await;
}

pub async fn export_reverse_config(
    options: ReverseExportOptions<'_>,
) -> Result<ReverseExportResult> {
    ensure_output_available(options.output)?;
    let alex_api_base = normalize_alex_api_base(options.alex_api_base)?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("building CLIProxyAPI reverse setup client")?;

    let capabilities = response_json(
        client
            .get(format!("{alex_api_base}/alex/capabilities"))
            .send()
            .await
            .context("probing Alex reverse capabilities")?,
        "probing Alex reverse capabilities",
    )
    .await?;
    let schema = validate_capabilities(&capabilities, options.cliproxyapi_version)?;

    let (key_id, key) = if let Some(key) = options.existing_key {
        (None, validate_scoped_key(key)?.to_string())
    } else {
        let admin_base = options
            .admin_base
            .context("a remote Alex export requires --key-file or ALEXANDRIA_HARNESS_KEY")?;
        let admin_key = options
            .admin_key
            .context("local Alex admin key is unavailable")?;
        let (id, key) = mint_reverse_key(&client, admin_base, admin_key).await?;
        (Some(id), key)
    };

    let result = async {
        let catalog = response_json(
            client
                .get(format!("{alex_api_base}/models"))
                .bearer_auth(&key)
                .send()
                .await
                .context("fetching the Alex model catalog")?,
            "fetching the Alex model catalog",
        )
        .await?;
        let models = select_reverse_models(&catalog, options.requested_models)?;
        let config =
            render_reverse_config(&alex_api_base, &key, &models, options.cliproxyapi_version)?;
        write_private(options.output, &config)?;
        Ok::<usize, anyhow::Error>(models.len())
    }
    .await;

    let model_count = match result {
        Ok(count) => count,
        Err(error) => {
            if let (Some(key_id), Some(admin_base), Some(admin_key)) =
                (key_id.as_deref(), options.admin_base, options.admin_key)
            {
                revoke_reverse_key(&client, admin_base, admin_key, key_id).await;
            }
            return Err(error);
        }
    };
    Ok(ReverseExportResult {
        output: options.output.to_path_buf(),
        model_count,
        key_id,
        schema,
        alex_api_base,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SCOPED_KEY: &str =
        "alxk-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn reverse_model_selection_is_deterministic_and_loop_safe() {
        let catalog = serde_json::json!({"data": [
            {"id": "gpt-5"},
            {"id": "alex/gpt-5"},
            {"id": "alex/claude-opus-4-8"},
            {"id": "alex/cliproxyapi/gpt-loop"},
            {"id": "alex/../escape"},
            {"id": "alex/gpt-5"}
        ]});
        assert_eq!(
            select_reverse_models(&catalog, &[]).unwrap(),
            vec!["alex/claude-opus-4-8", "alex/gpt-5"]
        );
        assert_eq!(
            select_reverse_models(&catalog, &["gpt-5".into()]).unwrap(),
            vec!["alex/gpt-5"]
        );
        assert!(select_reverse_models(&catalog, &["cliproxyapi/gpt-loop".into()]).is_err());
    }

    #[test]
    fn reverse_config_uses_v7_schema_headers_and_single_prefix() {
        let rendered = render_reverse_config(
            "http://127.0.0.1:8317/v1",
            TEST_SCOPED_KEY,
            &["alex/gpt-5".into(), "alex/claude-opus-4-8".into()],
            Some("v7.4.1"),
        )
        .unwrap();
        assert!(rendered.contains("passthrough-headers: true"));
        assert!(rendered.contains("prefix: \"alex\""));
        assert!(rendered.contains("name: \"alex/gpt-5\""));
        assert!(rendered.contains("alias: \"gpt-5\""));
        assert!(!rendered.contains("alias: \"alex/gpt-5\""));
        assert!(rendered.contains("X-Alexandria-Route-Chain: \"cliproxyapi\""));
        assert!(rendered.contains(alex_proxy::CLIPROXYAPI_REVERSE_SCHEMA));
    }

    #[test]
    fn reverse_url_and_version_validation_are_strict() {
        assert_eq!(
            normalize_alex_api_base("http://localhost:8317").unwrap(),
            "http://localhost:8317/v1"
        );
        assert!(normalize_alex_api_base("http://example.com/v1").is_err());
        assert!(normalize_alex_api_base("https://user:key@example.com/v1").is_err());
        let payload = serde_json::json!({"integrations": {"cliproxyapi_reverse": {
            "schema": alex_proxy::CLIPROXYAPI_REVERSE_SCHEMA,
            "minimum_cliproxyapi_major": 7,
            "capabilities": alex_proxy::CLIPROXYAPI_REVERSE_CAPABILITIES
        }}});
        assert!(validate_capabilities(&payload, Some("v7.0.0")).is_ok());
        assert!(validate_capabilities(&payload, Some("6.9.0")).is_err());
        let incomplete = serde_json::json!({"integrations": {"cliproxyapi_reverse": {
            "schema": alex_proxy::CLIPROXYAPI_REVERSE_SCHEMA,
            "minimum_cliproxyapi_major": 7,
            "capabilities": ["openai-chat"]
        }}});
        assert!(validate_capabilities(&incomplete, Some("v7.0.0")).is_err());
    }

    #[test]
    fn reverse_config_writer_is_private_and_never_overwrites() {
        let directory = std::env::temp_dir().join(format!(
            "alex-cliproxyapi-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.yaml");
        write_private(&path, TEST_SCOPED_KEY).unwrap();
        assert!(write_private(&path, "replacement").is_err());
        assert_eq!(read_private_key_file(&path).unwrap(), TEST_SCOPED_KEY);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[tokio::test]
    async fn reverse_export_negotiates_mints_and_writes_without_printing_secrets() {
        use axum::http::HeaderMap;
        use axum::routing::{get, post};
        use axum::Router;

        let service = Router::new()
            .route(
                "/v1/alex/capabilities",
                get(|| async {
                    axum::Json(serde_json::json!({"integrations": {"cliproxyapi_reverse": {
                        "schema": alex_proxy::CLIPROXYAPI_REVERSE_SCHEMA,
                        "minimum_cliproxyapi_major": 7,
                        "capabilities": alex_proxy::CLIPROXYAPI_REVERSE_CAPABILITIES
                    }}}))
                }),
            )
            .route(
                "/admin/run-keys",
                post(|headers: HeaderMap| async move {
                    assert_eq!(headers["x-api-key"], "admin-secret");
                    axum::Json(serde_json::json!({
                        "id": "rk-exported",
                        "key": TEST_SCOPED_KEY
                    }))
                }),
            )
            .route(
                "/v1/models",
                get(|headers: HeaderMap| async move {
                    assert_eq!(
                        headers["authorization"],
                        format!("Bearer {TEST_SCOPED_KEY}")
                    );
                    axum::Json(serde_json::json!({"data": [
                        {"id": "alex/gpt-5"},
                        {"id": "alex/cliproxyapi/rejected-loop"}
                    ]}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, service).await.unwrap() });
        let directory = std::env::temp_dir().join(format!(
            "alex-cliproxyapi-network-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let output = directory.join("alex-provider.yaml");
        let base = format!("http://{address}");
        let result = export_reverse_config(ReverseExportOptions {
            output: &output,
            alex_api_base: &base,
            existing_key: None,
            admin_base: Some(&base),
            admin_key: Some("admin-secret"),
            requested_models: &[],
            cliproxyapi_version: Some("v7.0.0"),
        })
        .await
        .unwrap();
        assert_eq!(result.key_id.as_deref(), Some("rk-exported"));
        assert_eq!(result.model_count, 1);
        let written = std::fs::read_to_string(&output).unwrap();
        assert!(written.contains(TEST_SCOPED_KEY));
        assert!(written.contains("name: \"alex/gpt-5\""));
        assert!(!written.contains("rejected-loop"));
        std::fs::remove_file(output).unwrap();
        std::fs::remove_dir(directory).unwrap();
        server.abort();
    }
}
