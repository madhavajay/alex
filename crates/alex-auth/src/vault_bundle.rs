//! Portable encrypted credential bundles.  Plaintext only exists while a
//! caller is explicitly exporting/importing it; blobs are authenticated before
//! they are decoded or written.
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, bail, Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::{now_ms, Account, Vault};

const BUNDLE_VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const MEM_KIB: u32 = 19_456;
const ITERATIONS: u32 = 2;
const PARALLELISM: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedVaultBlob {
    pub version: u8,
    pub kdf_params: KdfParams,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessCredential {
    pub harness: String,
    pub logical_name: String,
    pub bytes: Vec<u8>,
    pub mode: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultBundle {
    pub version: u8,
    pub created_ms: i64,
    pub accounts: Vec<Account>,
    pub harness_credentials: Vec<HarnessCredential>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BundleSelection {
    pub accounts: Option<Vec<String>>,
    pub harnesses: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportSummary {
    pub accounts: Vec<String>,
    pub harness_credentials: Vec<String>,
    pub oauth_overwritten: Vec<String>,
}

fn derive(passphrase: &str, salt: &[u8], params: &KdfParams) -> Result<[u8; 32]> {
    if passphrase.is_empty() {
        bail!("passphrase is required");
    }
    let params = Params::new(
        params.memory_kib,
        params.iterations,
        params.parallelism,
        Some(32),
    )
    .map_err(|_| anyhow!("invalid encryption parameters"))?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|_| anyhow!("could not derive encryption key"))?;
    Ok(key)
}

pub fn encrypt_bundle(bundle: &VaultBundle, passphrase: &str) -> Result<EncryptedVaultBlob> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce);
    let kdf_params = KdfParams {
        memory_kib: MEM_KIB,
        iterations: ITERATIONS,
        parallelism: PARALLELISM,
    };
    let key = derive(passphrase, &salt, &kdf_params)?;
    let plaintext = serde_json::to_vec(bundle)?;
    let ciphertext = Aes256Gcm::new_from_slice(&key)?
        .encrypt(Nonce::from_slice(&nonce), plaintext.as_ref())
        .map_err(|_| anyhow!("could not encrypt vault bundle"))?;
    Ok(EncryptedVaultBlob {
        version: BUNDLE_VERSION,
        kdf_params,
        salt: STANDARD.encode(salt),
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(ciphertext),
    })
}

pub fn decrypt_bundle(blob: &EncryptedVaultBlob, passphrase: &str) -> Result<VaultBundle> {
    if blob.version != BUNDLE_VERSION {
        bail!("unsupported vault bundle version {}", blob.version);
    }
    let salt = STANDARD.decode(&blob.salt).context("invalid bundle salt")?;
    let nonce = STANDARD
        .decode(&blob.nonce)
        .context("invalid bundle nonce")?;
    if salt.len() != SALT_LEN || nonce.len() != NONCE_LEN {
        bail!("invalid vault bundle parameters");
    }
    let key = derive(passphrase, &salt, &blob.kdf_params)?;
    let ciphertext = STANDARD
        .decode(&blob.ciphertext)
        .context("invalid bundle ciphertext")?;
    let plaintext = Aes256Gcm::new_from_slice(&key)?
        .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| {
            anyhow!("could not decrypt vault bundle (wrong passphrase or corrupted blob)")
        })?;
    let bundle: VaultBundle =
        serde_json::from_slice(&plaintext).context("invalid vault bundle payload")?;
    if bundle.version != BUNDLE_VERSION {
        bail!(
            "unsupported vault bundle payload version {}",
            bundle.version
        );
    }
    Ok(bundle)
}

/// Logical path registry.  The manifest deliberately never contains these
/// host-specific paths: import resolves them for the receiving OS/home.
pub fn harness_cred_paths(harness: &str) -> Vec<(String, PathBuf)> {
    harness_cred_paths_in(
        harness,
        &dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
    )
}

pub fn harness_cred_paths_in(harness: &str, home: &Path) -> Vec<(String, PathBuf)> {
    let mac = cfg!(target_os = "macos");
    let p = |name: &str, path: PathBuf| vec![(name.to_string(), path)];
    match harness {
        "amp" => p(
            "session",
            if mac {
                home.join("Library/Application Support/amp/session.json")
            } else {
                home.join(".local/share/amp/session.json")
            },
        ),
        "cursor" => p(
            "auth",
            if mac {
                home.join("Library/Application Support/cursor/auth.json")
            } else {
                home.join(".config/cursor/auth.json")
            },
        ),
        "droid" | "factory" => vec![
            ("auth.v2.key".into(), home.join(".factory/auth.v2.key")),
            ("auth.v2.file".into(), home.join(".factory/auth.v2.file")),
        ],
        "codex" => p("auth", home.join(".codex/auth.json")),
        "claude" => p("credentials", home.join(".claude/.credentials.json")),
        "grok" => p("auth", home.join(".grok/auth.json")),
        _ => vec![],
    }
}

pub const HARNESS_NAMES: &[&str] = &["amp", "cursor", "droid", "codex", "claude", "grok"];

fn selected(value: &str, selection: &Option<Vec<String>>) -> bool {
    selection
        .as_ref()
        .map(|items| items.iter().any(|i| i == "all" || i == value))
        .unwrap_or(true)
}

pub async fn export_bundle(vault: &Vault, selection: BundleSelection) -> Result<VaultBundle> {
    export_bundle_in(
        vault,
        selection,
        &dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
    )
    .await
}

pub async fn export_bundle_in(
    vault: &Vault,
    selection: BundleSelection,
    home: &Path,
) -> Result<VaultBundle> {
    let accounts = vault
        .list()
        .await
        .into_iter()
        .filter(|a| selected(&a.id, &selection.accounts))
        .collect();
    let mut harness_credentials = Vec::new();
    for harness in HARNESS_NAMES {
        if !selected(harness, &selection.harnesses) {
            continue;
        }
        for (logical_name, path) in harness_cred_paths_in(harness, home) {
            if let Ok(bytes) = std::fs::read(&path) {
                harness_credentials.push(HarnessCredential {
                    harness: (*harness).into(),
                    logical_name,
                    bytes,
                    mode: file_mode(&path),
                });
            }
        }
    }
    Ok(VaultBundle {
        version: BUNDLE_VERSION,
        created_ms: now_ms(),
        accounts,
        harness_credentials,
    })
}

pub async fn import_bundle(vault: &Vault, bundle: VaultBundle) -> Result<ImportSummary> {
    import_bundle_in(
        vault,
        bundle,
        &dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
    )
    .await
}

pub async fn import_bundle_in(
    vault: &Vault,
    bundle: VaultBundle,
    home: &Path,
) -> Result<ImportSummary> {
    let mut summary = ImportSummary {
        accounts: vec![],
        harness_credentials: vec![],
        oauth_overwritten: vec![],
    };
    let existing: std::collections::HashSet<String> =
        vault.list().await.into_iter().map(|a| a.id).collect();
    for mut account in bundle.accounts {
        if existing.contains(&account.id) && account.kind == "oauth" {
            summary.oauth_overwritten.push(account.id.clone());
        }
        account.path = None;
        vault.upsert(account.clone()).await?;
        summary.accounts.push(account.id);
    }
    for credential in bundle.harness_credentials {
        let Some((_, destination)) = harness_cred_paths_in(&credential.harness, home)
            .into_iter()
            .find(|(name, _)| name == &credential.logical_name)
        else {
            continue;
        };
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if destination.exists() {
            std::fs::copy(&destination, destination.with_extension("bak"))?;
        }
        std::fs::write(&destination, &credential.bytes)?;
        set_mode(&destination, credential.mode);
        summary.harness_credentials.push(format!(
            "{}/{}",
            credential.harness, credential.logical_name
        ));
    }
    Ok(summary)
}

#[cfg(unix)]
fn file_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0o600)
}
#[cfg(not(unix))]
fn file_mode(_: &Path) -> u32 {
    0o600
}
#[cfg(unix)]
fn set_mode(path: &Path, _: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypt_round_trip_and_wrong_passphrase_fails() {
        let bundle = VaultBundle {
            version: 1,
            created_ms: 1,
            accounts: vec![],
            harness_credentials: vec![],
        };
        let blob = encrypt_bundle(&bundle, "correct horse").unwrap();
        assert_eq!(
            decrypt_bundle(&blob, "correct horse").unwrap().created_ms,
            1
        );
        assert!(decrypt_bundle(&blob, "wrong").is_err());
    }

    #[tokio::test]
    async fn export_import_round_trip_writes_local_harness_paths_and_backup() {
        let root = std::env::temp_dir().join(format!("alex-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source_home = root.join("source-home");
        let target_home = root.join("target-home");
        let source = Vault::open(root.join("source-vault")).unwrap();
        let account = Account {
            id: "openai-oauth".into(),
            provider: alex_core::Provider::Openai,
            kind: "oauth".into(),
            name: "default".into(),
            description: None,
            paused: false,
            label: None,
            access_token: Some("source-token".into()),
            refresh_token: Some("refresh".into()),
            id_token: None,
            api_key: None,
            expires_at_ms: None,
            last_refresh_ms: None,
            account_meta: serde_json::Value::Null,
            cooldown_until_ms: None,
            status: "active".into(),
            path: None,
        };
        source.upsert(account).await.unwrap();
        let (_, source_path) = harness_cred_paths_in("codex", &source_home).pop().unwrap();
        std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        std::fs::write(&source_path, b"source-auth").unwrap();
        let bundle = export_bundle_in(&source, BundleSelection::default(), &source_home)
            .await
            .unwrap();
        let target = Vault::open(root.join("target-vault")).unwrap();
        let (_, target_path) = harness_cred_paths_in("codex", &target_home).pop().unwrap();
        std::fs::create_dir_all(target_path.parent().unwrap()).unwrap();
        std::fs::write(&target_path, b"old-auth").unwrap();
        let summary = import_bundle_in(&target, bundle, &target_home)
            .await
            .unwrap();
        assert_eq!(summary.accounts, vec!["openai-oauth"]);
        assert_eq!(std::fs::read(&target_path).unwrap(), b"source-auth");
        assert_eq!(
            std::fs::read(target_path.with_extension("bak")).unwrap(),
            b"old-auth"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(target_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
