//! Telegram long-poll transport for the inbound command bus.

use std::sync::Arc;
use std::time::Duration;

use alex_proxy::notify::{telegram_get_updates_url, NotificationDispatcher, WebhookFormat};
use serde::Deserialize;

use crate::commands::{daemon_command_router, DaemonCommandContext};
use crate::Config;

#[derive(Debug, Deserialize)]
struct TelegramUpdates {
    ok: bool,
    #[serde(default)]
    result: Vec<TelegramUpdate>,
}

#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramMessage>,
    channel_post: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    chat: TelegramChat,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
}

#[derive(Clone, PartialEq, Eq)]
struct CommandChannel {
    id: String,
    token: String,
    chat_id: String,
}

fn command_channels(config: &Config) -> Vec<CommandChannel> {
    config
        .notifications
        .iter()
        .enumerate()
        .filter(|(_, channel)| {
            channel.allow_commands && matches!(channel.format, WebhookFormat::Telegram)
        })
        .filter_map(|(index, channel)| {
            let token = channel
                .token
                .as_deref()
                .filter(|token| !token.trim().is_empty())?;
            let chat_id = channel
                .chat_id
                .as_deref()
                .filter(|chat_id| !chat_id.trim().is_empty())?;
            Some(CommandChannel {
                id: channel
                    .id
                    .clone()
                    .unwrap_or_else(|| format!("channel-{index}")),
                token: token.trim().to_string(),
                chat_id: chat_id.trim().to_string(),
            })
        })
        .collect()
}

/// Reconcile command pollers against the shared hot-updated config. This is
/// intentionally a supervisor rather than a startup-only snapshot: enabling
/// `allow_commands` through the admin API must make `/status` and paste-back
/// work immediately, without a daemon restart.
pub(crate) fn spawn_command_poller_supervisor(
    config: Arc<std::sync::Mutex<Config>>,
    dispatchers: Arc<std::sync::RwLock<NotificationDispatcher>>,
) -> (tokio::task::JoinHandle<()>, usize) {
    let initial_count = config
        .lock()
        .map(|config| command_channels(&config).len())
        .unwrap_or(0);
    let router = Arc::new(daemon_command_router());
    let supervisor = tokio::spawn(async move {
        let mut running: std::collections::HashMap<
            String,
            (CommandChannel, tokio::task::JoinHandle<()>),
        > = std::collections::HashMap::new();
        loop {
            let snapshot = config.lock().ok().map(|config| config.clone());
            let wanted = snapshot.as_ref().map(command_channels).unwrap_or_default();
            let wanted_ids: std::collections::HashSet<_> =
                wanted.iter().map(|channel| channel.id.clone()).collect();
            running.retain(|id, (_, task)| {
                let keep = wanted_ids.contains(id);
                if !keep {
                    task.abort();
                }
                keep
            });
            for channel in wanted {
                let unchanged = running
                    .get(&channel.id)
                    .is_some_and(|(active, task)| active == &channel && !task.is_finished());
                if unchanged {
                    continue;
                }
                if let Some((_, old)) = running.remove(&channel.id) {
                    old.abort();
                }
                let context = Arc::new(DaemonCommandContext::new(
                    snapshot
                        .clone()
                        .expect("wanted channels require a config snapshot"),
                    channel.id.clone(),
                ));
                let task = tokio::spawn(poll_channel(
                    channel.clone(),
                    router.clone(),
                    context,
                    dispatchers.clone(),
                ));
                running.insert(channel.id.clone(), (channel, task));
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    (supervisor, initial_count)
}

async fn poll_channel(
    channel: CommandChannel,
    router: Arc<crate::commands::CommandRouter>,
    context: Arc<DaemonCommandContext>,
    dispatchers: Arc<std::sync::RwLock<NotificationDispatcher>>,
) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(40))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(channel = %channel.id, %error, "telegram command client failed to initialize");
            if let Ok(dispatcher) = dispatchers.read() {
                dispatcher.record_poll_error(
                    &channel.id,
                    "telegram command client initialization failed",
                );
            }
            return;
        }
    };
    let url = telegram_get_updates_url(&channel.token);
    let mut offset = 0i64;
    // Let the local admin server begin accepting requests before a pending
    // `/status` update asks the shared aggregator to call it.
    tokio::time::sleep(Duration::from_millis(500)).await;

    loop {
        let response = client
            .get(&url)
            .query(&[
                ("offset", offset.to_string()),
                ("timeout", "30".to_string()),
                (
                    "allowed_updates",
                    "[\"message\",\"channel_post\"]".to_string(),
                ),
            ])
            .send()
            .await;
        let updates = match response {
            Ok(response) if response.status().is_success() => {
                response.json::<TelegramUpdates>().await.ok()
            }
            _ => None,
        };
        let Some(updates) = updates.filter(|updates| updates.ok) else {
            tracing::warn!(channel = %channel.id, "telegram command poll failed; retrying");
            if let Ok(dispatcher) = dispatchers.read() {
                dispatcher.record_poll_error(&channel.id, "Telegram getUpdates failed");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        if let Ok(dispatcher) = dispatchers.read() {
            dispatcher.record_poll_success(&channel.id);
        }

        for update in updates.result {
            // Advance the offset for every update, including rejected chats, so
            // an attacker cannot pin the control channel on a foreign update.
            offset = offset.max(update.update_id.saturating_add(1));
            let message = update.message.or(update.channel_post);
            let Some(message) = message else {
                continue;
            };
            if !chat_id_allowed(&channel.chat_id, message.chat.id) {
                tracing::warn!(
                    channel = %channel.id,
                    chat_id = message.chat.id,
                    "ignored telegram command from non-allowlisted chat"
                );
                if let Ok(dispatcher) = dispatchers.read() {
                    dispatcher.record_inbound(
                        &channel.id,
                        "[non-allowlisted update]",
                        false,
                        Some("ignored command from non-allowlisted chat"),
                    );
                }
                continue;
            }
            let Some(text) =
                allowlisted_text(&channel.chat_id, message.chat.id, message.text.as_deref())
            else {
                continue;
            };
            let reply = router.dispatch(context.as_ref(), text).await;
            let dispatch_error = command_reply_error(&reply);
            if let Ok(dispatcher) = dispatchers.read() {
                dispatcher.record_inbound(
                    &channel.id,
                    text,
                    dispatch_error.is_none(),
                    dispatch_error,
                );
            }
            let dispatcher = dispatchers.read().ok().map(|dispatcher| dispatcher.clone());
            let reply_failed = match dispatcher {
                Some(dispatcher) => dispatcher
                    .send_telegram_reply(&channel.id, &reply)
                    .await
                    .is_err(),
                None => true,
            };
            if reply_failed {
                // Delivery errors can contain the token-bearing request URL.
                tracing::warn!(channel = %channel.id, "telegram command reply failed");
            }
        }
    }
}

fn command_reply_error(reply: &str) -> Option<&'static str> {
    let lower = reply.to_ascii_lowercase();
    (lower.starts_with("could not") || lower.contains("unavailable"))
        .then_some("command dispatch failed")
}

pub(crate) fn chat_id_allowed(configured: &str, incoming: i64) -> bool {
    configured.trim() == incoming.to_string()
}

fn allowlisted_text<'a>(configured: &str, incoming: i64, text: Option<&'a str>) -> Option<&'a str> {
    chat_id_allowed(configured, incoming)
        .then_some(text)
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_id_allowlist_rejects_foreign_chat() {
        assert!(chat_id_allowed("-100123", -100123));
        assert!(!chat_id_allowed("-100123", -100999));
        assert!(!chat_id_allowed("not-a-chat", -100123));
        assert_eq!(
            allowlisted_text("42", 42, Some("code#state")),
            Some("code#state")
        );
        assert_eq!(allowlisted_text("42", 99, Some("code#state")), None);
    }

    #[test]
    fn command_channel_reconciliation_follows_hot_allow_commands_setting() {
        let mut config: Config = toml::from_str(
            r#"
                host = "127.0.0.1"
                port = 4100
                data_dir = "/tmp/alex-telegram-reconcile"
                local_key = "alx-local"
            "#,
        )
        .unwrap();
        config
            .notifications
            .push(alex_proxy::notify::NotificationChannelConfig {
                id: Some("control".into()),
                format: WebhookFormat::Telegram,
                token: Some("123:secret".into()),
                chat_id: Some("42".into()),
                allow_commands: false,
                ..Default::default()
            });
        assert!(command_channels(&config).is_empty());
        config.notifications[0].allow_commands = true;
        assert_eq!(command_channels(&config)[0].id, "control");
        config.notifications[0].allow_commands = false;
        assert!(command_channels(&config).is_empty());
    }

    #[test]
    fn command_dispatch_failures_are_classified_for_channel_visibility() {
        assert_eq!(
            command_reply_error("status unavailable; try again shortly"),
            Some("command dispatch failed")
        );
        assert_eq!(
            command_reply_error("Could not submit the code"),
            Some("command dispatch failed")
        );
        assert_eq!(command_reply_error("pong"), None);
    }
}
