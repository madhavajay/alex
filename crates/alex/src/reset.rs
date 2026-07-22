use std::path::{Path, PathBuf};
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
    progress: Option<&alex_proxy::AppState>,
) -> Result<Value> {
    if let Some(state) = progress {
        state.set_reset_progress("counting_bodies", "Counting captured bodies");
    }
    let counts = store.reset_counts()?;
    if let Some(state) = progress {
        state.set_reset_progress("counting_traces", "Counting traces and accounts");
    }
    let accounts = vault.list().await;
    if let Some(state) = progress {
        state.set_reset_progress("counting_harnesses", "Checking connected harnesses");
    }
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
    execute_with_progress(config, config_path, vault, store, selection, dry_run, None).await
}

async fn execute_with_progress(
    config: &Config,
    config_path: &Path,
    vault: &Vault,
    store: &Store,
    selection: Selection,
    dry_run: bool,
    progress: Option<&alex_proxy::AppState>,
) -> Result<Value> {
    let mut result = plan(config, vault, store, selection, dry_run, progress).await?;
    if dry_run || !selection.any() {
        return Ok(result);
    }

    // Disconnect before rotating local_key. The shared disconnect command is
    // the single owner of harness-local file cleanup and harness-key revocation.
    if selection.harnesses {
        let harnesses = connected_harnesses(config).await?;
        let total = harnesses.len();
        for (index, harness) in harnesses.into_iter().enumerate() {
            if let Some(state) = progress {
                state.set_reset_progress(
                    "disconnecting_harnesses",
                    format!("Disconnecting harnesses ({}/{total})", index + 1),
                );
            }
            harness_connect::disconnect_cmd(config, harness, None).await?;
        }
    }

    if selection.credentials {
        if let Some(state) = progress {
            state.set_reset_progress("removing_accounts", "Removing provider accounts");
        }
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
        if let Some(state) = progress {
            state.set_reset_progress("clearing_traces", "Removing traces and captured bodies");
        }
        store.clear_traces_and_bodies()?;
    }
    if selection.cache {
        if let Some(state) = progress {
            state.set_reset_progress("clearing_caches", "Clearing derived caches");
        }
        store.clear_derived_cache()?;
    }
    if selection.settings {
        if let Some(state) = progress {
            state.set_reset_progress("restoring_settings", "Restoring default settings");
        }
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

pub(crate) struct DaemonResetHandler {
    config: Arc<std::sync::Mutex<Config>>,
    config_path: PathBuf,
}

impl DaemonResetHandler {
    pub(crate) fn new(config: Arc<std::sync::Mutex<Config>>, config_path: PathBuf) -> Self {
        Self {
            config,
            config_path,
        }
    }
}

impl alex_proxy::ResetHandler for DaemonResetHandler {
    fn reset(
        &self,
        state: Arc<alex_proxy::AppState>,
        request: alex_proxy::ResetRequest,
    ) -> alex_proxy::ResetFuture {
        let shared_config = self.config.clone();
        let config_path = self.config_path.clone();
        Box::pin(async move {
            let config = shared_config
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let result = execute_with_progress(
                &config,
                &config_path,
                state.vault.as_ref(),
                state.store.as_ref(),
                request.clone().into(),
                request.dry_run,
                Some(state.as_ref()),
            )
            .await?;
            if !request.dry_run && request.credentials {
                state.run_keys.write().unwrap().clear();
                alex_proxy::reset_auth_sessions(&state).await;
            }
            if !request.dry_run && request.settings {
                let updated: Config = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
                *state.local_key.write().unwrap() = updated.local_key.clone();
                *shared_config
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = updated;
                alex_proxy::reset_notification_state(&state)?;
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
    use alex_store::ToolCallRecord;
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
            .upsert_tool_call(&ToolCallRecord {
                id: "tool-1".into(),
                harness: "codex".into(),
                session_id: "session-1".into(),
                turn_id: None,
                tool_call_id: "call-1".into(),
                trace_id: Some("trace-1".into()),
                tool_name: "shell".into(),
                ts_start_ms: 1,
                ts_end_ms: Some(2),
                is_error: Some(false),
                exit_status: Some(0),
                args_body_path: None,
                result_body_path: None,
            })
            .unwrap();
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
        assert!(store.session_tool_calls("session-1").unwrap().is_empty());
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

    #[tokio::test]
    async fn reset_all_clears_live_state_without_reimporting_or_deleting_source_credentials() {
        let (mut config, path, vault, store) = fixture("all-live-state").await;
        config.notifications = vec![alex_proxy::notify::NotificationChannelConfig {
            id: Some("telegram".into()),
            url: "https://api.telegram.org/bot123:secret/sendMessage".into(),
            allow_commands: true,
            ..Default::default()
        }];
        save_config_at(&config, &path).unwrap();

        let source_path = config.data_dir.join(".codex/auth.json");
        std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        let source_bytes = br#"{"tokens":{"access_token":"external-secret"}}"#;
        std::fs::write(&source_path, source_bytes).unwrap();
        let activity_path = config.data_dir.join("notification-messages.json");
        std::fs::write(
            &activity_path,
            br#"[{"ts":1,"direction":"in","channel_id":"telegram","kind":"command","ok":true,"error":null,"summary":"/status"}]"#,
        )
        .unwrap();

        let shared_config = Arc::new(std::sync::Mutex::new(config.clone()));
        let state = alex_proxy::build_state(
            config.local_key.clone(),
            Arc::new(vault),
            Arc::new(store),
            None,
            config.base_url(),
            config.upstream_stream_idle_timeout(),
        );
        alex_proxy::set_notifications(&state, config.notification_settings());
        let handler = DaemonResetHandler::new(shared_config.clone(), path.clone());
        alex_proxy::ResetHandler::reset(
            &handler,
            state.clone(),
            alex_proxy::ResetRequest {
                credentials: true,
                settings: true,
                traces: true,
                harnesses: true,
                cache: true,
                dry_run: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert!(state.vault.list().await.is_empty());
        let counts = state.store.reset_counts().unwrap();
        assert_eq!(
            (counts.traces, counts.heartbeats, counts.run_keys),
            (0, 0, 0)
        );
        assert!(state
            .store
            .session_tool_calls("session-1")
            .unwrap()
            .is_empty());
        assert!(!activity_path.exists());
        assert!(shared_config.lock().unwrap().notifications.is_empty());
        assert!(
            alex_auth::detect_import_candidates_in(&config.data_dir, None)
                .iter()
                .any(|candidate| candidate.source == "codex"),
            "external credentials remain candidates but are not auto-imported"
        );
        assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);
        assert!(
            state.vault.list().await.is_empty(),
            "detection is non-mutating"
        );
    }
}
