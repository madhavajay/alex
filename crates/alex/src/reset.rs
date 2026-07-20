use std::path::Path;
use std::sync::Arc;

use alex_auth::Vault;
use alex_store::{KnownAccount, Store};
use anyhow::Result;
use serde_json::{json, Value};

use crate::{harness_connect, random_key, save_config_at, Config};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Selection {
    pub(crate) credentials: bool,
    pub(crate) settings: bool,
    pub(crate) traces: bool,
    pub(crate) harnesses: bool,
    pub(crate) cache: bool,
}

impl Selection {
    fn selected(self) -> Vec<&'static str> {
        let mut selected = Vec::new();
        if self.credentials {
            selected.push("credentials");
        }
        if self.settings {
            selected.push("settings");
        }
        if self.traces {
            selected.push("traces");
        }
        if self.harnesses {
            selected.push("harnesses");
        }
        if self.cache {
            selected.push("cache");
        }
        selected
    }

    fn any(self) -> bool {
        self.credentials || self.settings || self.traces || self.harnesses || self.cache
    }
}

impl From<alex_proxy::ResetRequest> for Selection {
    fn from(request: alex_proxy::ResetRequest) -> Self {
        Self {
            credentials: request.credentials,
            settings: request.settings,
            traces: request.traces,
            harnesses: request.harnesses,
            cache: request.cache,
        }
    }
}

fn account_known(account: &alex_auth::Account) -> KnownAccount {
    KnownAccount::new(
        account.id.clone(),
        account.provider.as_str(),
        account.name.clone(),
        account.kind.clone(),
        account.subscription_identity(),
        account.email(),
    )
}

async fn connected_harnesses(config: &Config) -> Result<Vec<String>> {
    Ok(harness_connect::harness_statuses(config, None, false)
        .await?
        .into_iter()
        .filter(|status| status.connected && status.supports_connect)
        .map(|status| status.name.to_string())
        .collect())
}

async fn plan(
    config: &Config,
    vault: &Vault,
    store: &Store,
    selection: Selection,
    dry_run: bool,
) -> Result<Value> {
    let counts = store.reset_counts()?;
    let accounts = vault.list().await;
    let harnesses = connected_harnesses(config).await?;
    let selected = selection.selected();
    Ok(json!({
        "dry_run": dry_run,
        "applied": false,
        "selected": selected,
        "counts": {
            "accounts": accounts.len(),
            "run_keys": counts.run_keys,
            "traces": counts.traces,
            "heartbeats": counts.heartbeats,
            "bodies": {"files": counts.body_files, "bytes": counts.body_bytes},
            "connected_harnesses": harnesses.len(),
            "pricing": counts.pricing,
            "dario_prompt_cache": {"files": counts.dario_prompt_cache_files, "bytes": counts.dario_prompt_cache_bytes},
        },
        "harnesses": harnesses,
        "actions": {
            "credentials": selection.credentials.then_some("remove account JSON; retain removed-accounts tombstones and known_accounts; revoke active run keys"),
            "settings": selection.settings.then_some("restore config.toml defaults; preserve update_channel and preserve local_key unless harnesses is selected"),
            "traces": selection.traces.then_some("delete traces and heartbeats; remove data_dir/bodies recursively"),
            "harnesses": selection.harnesses.then_some("disconnect each connected harness through alex harness disconnect"),
            "cache": selection.cache.then_some("delete derived pricing and dario prompt caches"),
        },
        "settings": {
            "preserves_update_channel": selection.settings,
            "preserves_local_key": selection.settings && !selection.harnesses,
            "rotates_local_key": selection.settings && selection.harnesses,
        },
    }))
}

pub(crate) async fn execute(
    config: &Config,
    config_path: &Path,
    vault: &Vault,
    store: &Store,
    selection: Selection,
    dry_run: bool,
) -> Result<Value> {
    let mut result = plan(config, vault, store, selection, dry_run).await?;
    if dry_run || !selection.any() {
        return Ok(result);
    }

    // Disconnect before rotating local_key. The shared disconnect command is
    // the single owner of harness-local file cleanup and harness-key revocation.
    if selection.harnesses {
        for harness in connected_harnesses(config).await? {
            harness_connect::disconnect_cmd(config, harness, None).await?;
        }
    }

    if selection.credentials {
        for account in vault.list().await {
            let known = account_known(&account);
            if vault.remove(&account.id).await? {
                store.tombstone_known_account(&known, alex_auth::now_ms())?;
            }
        }
        // Credential reset owns provider/run credentials. Harness connection
        // credentials remain under the separate `harnesses` reset selection.
        store.revoke_all_run_keys(false)?;
    }
    if selection.traces {
        store.clear_traces_and_bodies()?;
    }
    if selection.cache {
        store.clear_derived_cache()?;
    }
    if selection.settings {
        // data_dir is the location of the data being reset, not a preference;
        // retaining it avoids silently moving a configured installation to a
        // second, empty data directory.
        let mut reset = Config::defaults_for(config.data_dir.clone());
        reset.update_channel = config.update_channel.clone();
        if !selection.harnesses {
            reset.local_key = config.local_key.clone();
        } else {
            reset.local_key = random_key("alx");
        }
        save_config_at(&reset, config_path)?;
    }
    result["dry_run"] = json!(false);
    result["applied"] = json!(true);
    Ok(result)
}

pub(crate) struct DaemonResetHandler;

impl alex_proxy::ResetHandler for DaemonResetHandler {
    fn reset(
        &self,
        state: Arc<alex_proxy::AppState>,
        request: alex_proxy::ResetRequest,
    ) -> alex_proxy::ResetFuture {
        Box::pin(async move {
            let (config, _) = crate::load_or_create_config()?;
            let result = execute(
                &config,
                &crate::alexandria_home().join("config.toml"),
                state.vault.as_ref(),
                state.store.as_ref(),
                request.clone().into(),
                request.dry_run,
            )
            .await?;
            if !request.dry_run && request.credentials {
                state.run_keys.write().unwrap().clear();
            }
            if !request.dry_run && request.settings && request.harnesses {
                let (updated, _) = crate::load_or_create_config()?;
                *state.local_key.write().unwrap() = updated.local_key;
            }
            Ok(result)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alex_auth::Account;
    use alex_core::{Provider, TraceRecord};
    use std::path::PathBuf;

    fn tmpdir(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("alex-reset-{name}-{nonce}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn account() -> Account {
        Account {
            id: "openai-oauth-default".into(),
            provider: Provider::Openai,
            kind: "oauth".into(),
            name: "default".into(),
            description: None,
            paused: false,
            label: None,
            access_token: Some("secret".into()),
            refresh_token: None,
            id_token: None,
            api_key: None,
            expires_at_ms: None,
            last_refresh_ms: None,
            account_meta: json!({"email": "a@example.test"}),
            cooldown_until_ms: None,
            status: "active".into(),
            path: None,
        }
    }

    fn trace() -> TraceRecord {
        TraceRecord {
            id: "trace-1".into(),
            ts_request_ms: 1,
            ..TraceRecord::default()
        }
    }

    async fn fixture(name: &str) -> (Config, PathBuf, Vault, Store) {
        let home = tmpdir(name);
        let mut config = Config::defaults_for(home.clone());
        // Reset tests exercise offline cleanup.  Never accidentally talk to a
        // developer's daemon on the default port while doing so.
        config.port = 0;
        // Harness detection resolves config dirs under the developer's real
        // home unless overridden; a reset execute() with harnesses selected
        // would then disconnect their live claude/codex/amp integrations.
        // Pin every harness into this test's sandbox so that can never happen.
        for spec in harness_connect::HARNESSES {
            config.harness_overrides.insert(
                spec.name.into(),
                crate::HarnessOverride {
                    binary: Some(home.join("no-such-binary")),
                    config_dir: Some(home.join("harness").join(spec.name)),
                },
            );
        }
        let config_path = home.join("config.toml");
        save_config_at(&config, &config_path).unwrap();
        let vault = Vault::open(home.join("accounts")).unwrap();
        vault.upsert(account()).await.unwrap();
        let store = Store::open(home.clone()).unwrap();
        store.insert_trace(&trace()).unwrap();
        store.write_body("trace-1", "request", b"body").unwrap();
        store
            .insert_heartbeat(1, "openai", None, true, Some(200), 1, "ok")
            .unwrap();
        store
            .insert_run_key("rk-test", "hash", "run", None, None, None, 1, None)
            .unwrap();
        std::fs::create_dir_all(home.join("dario-prompt-cache")).unwrap();
        std::fs::write(home.join("dario-prompt-cache/cache.json"), b"cache").unwrap();
        (config, config_path, vault, store)
    }

    #[tokio::test]
    async fn dry_run_changes_nothing_and_reports_real_counts() {
        let (config, path, vault, store) = fixture("dry-run").await;
        let out = execute(
            &config,
            &path,
            &vault,
            &store,
            Selection {
                traces: true,
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap();
        assert_eq!(out["counts"]["traces"], 1);
        assert_eq!(out["counts"]["bodies"]["files"], 1);
        assert_eq!(store.reset_counts().unwrap().traces, 1);
        assert_eq!(vault.list().await.len(), 1);
    }

    #[tokio::test]
    async fn credentials_preserve_tombstones_and_known_accounts() {
        let (config, path, vault, store) = fixture("credentials").await;
        store
            .upsert_known_account(&account_known(&account()))
            .unwrap();
        execute(
            &config,
            &path,
            &vault,
            &store,
            Selection {
                credentials: true,
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap();
        assert!(vault.list().await.is_empty());
        assert!(config
            .data_dir
            .join("accounts/removed-accounts/openai-oauth-default.json")
            .exists());
        assert_eq!(store.list_known_accounts().unwrap().len(), 1);
        assert_eq!(store.reset_counts().unwrap().run_keys, 0);
        assert_eq!(store.reset_counts().unwrap().traces, 1);
    }

    #[tokio::test]
    async fn traces_remove_rows_heartbeats_and_body_files_only() {
        let (config, path, vault, store) = fixture("traces").await;
        execute(
            &config,
            &path,
            &vault,
            &store,
            Selection {
                traces: true,
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap();
        let counts = store.reset_counts().unwrap();
        assert_eq!(
            (counts.traces, counts.heartbeats, counts.body_files),
            (0, 0, 0)
        );
        assert_eq!(vault.list().await.len(), 1);
        assert_eq!(counts.run_keys, 1);
    }

    #[tokio::test]
    async fn settings_preserve_channel_and_key_unless_harnesses_selected() {
        let (mut config, path, vault, store) = fixture("settings").await;
        config.update_channel = "beta".into();
        config.local_key = "old-key".into();
        config.host = "0.0.0.0".into();
        save_config_at(&config, &path).unwrap();
        execute(
            &config,
            &path,
            &vault,
            &store,
            Selection {
                settings: true,
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap();
        let after: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after.update_channel, "beta");
        assert_eq!(after.local_key, "old-key");
        assert_eq!(after.host, "127.0.0.1");

        let mut with_empty_harness_dir = after.clone();
        // The settings reset above wrote defaults to disk, dropping the
        // fixture's sandbox overrides; without restoring them the harness
        // disconnect below resolves claude/codex/amp to the developer's REAL
        // home config and disconnects their live integrations.
        with_empty_harness_dir.harness_overrides = config.harness_overrides.clone();
        with_empty_harness_dir.harness_overrides.insert(
            "pi".into(),
            crate::HarnessOverride {
                binary: None,
                config_dir: Some(config.data_dir.join("empty-pi")),
            },
        );
        execute(
            &with_empty_harness_dir,
            &path,
            &vault,
            &store,
            Selection {
                settings: true,
                harnesses: true,
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap();
        let rotated: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_ne!(rotated.local_key, "old-key");
    }

    #[tokio::test]
    async fn each_other_category_leaves_unselected_data_alone() {
        let (config, path, vault, store) = fixture("cache").await;
        execute(
            &config,
            &path,
            &vault,
            &store,
            Selection {
                cache: true,
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap();
        let counts = store.reset_counts().unwrap();
        // A cache reset drops LEARNED prices and restores the bundled catalog -- it
        // must not leave `pricing` empty. That table is also the model catalog served
        // by /v1/models and written into every harness, so an empty one silently
        // dropped models (claude-fable-5) out of the harnesses until the daemon was
        // restarted. Asserting 0 here is what encoded that bug.
        assert!(
            counts.pricing > 0,
            "cache reset must re-seed the model catalog, not empty it"
        );
        assert_eq!(counts.dario_prompt_cache_files, 0);
        assert_eq!(counts.traces, 1);
        assert_eq!(vault.list().await.len(), 1);
    }

    #[tokio::test]
    async fn harnesses_use_disconnect_and_leave_other_categories_alone() {
        let (mut config, path, vault, store) = fixture("harnesses").await;
        let pi_dir = config.data_dir.join("pi");
        config.harness_overrides.insert(
            "pi".into(),
            crate::HarnessOverride {
                binary: None,
                config_dir: Some(pi_dir.clone()),
            },
        );
        std::fs::create_dir_all(&pi_dir).unwrap();
        let models = vec!["alex/test".into()];
        harness_connect::upsert_pi_provider(
            &pi_dir.join("models.json"),
            "http://127.0.0.1:4100",
            "harness-key",
            &models,
        )
        .unwrap();
        store
            .insert_run_key(
                "rk-pi",
                "hash-pi",
                "harness",
                None,
                None,
                Some("pi"),
                1,
                None,
            )
            .unwrap();
        execute(
            &config,
            &path,
            &vault,
            &store,
            Selection {
                harnesses: true,
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap();
        assert!(!harness_connect::read_pi_model_ids(&pi_dir).contains(&"alex/test".to_string()));
        assert_eq!(
            store.reset_counts().unwrap().run_keys,
            1,
            "the ordinary run key survives"
        );
        assert_eq!(store.reset_counts().unwrap().traces, 1);
        assert_eq!(vault.list().await.len(), 1);
    }
}
