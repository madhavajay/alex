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

#[derive(Clone)]
struct CommandChannel {
    id: String,
    token: String,
    chat_id: String,
}

pub(crate) fn spawn_command_pollers(
    config: &Config,
    dispatcher: NotificationDispatcher,
) -> Vec<tokio::task::JoinHandle<()>> {
    let channels: Vec<CommandChannel> = config
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
        .collect();
    if channels.is_empty() {
        return Vec::new();
    }

    let router = Arc::new(daemon_command_router());
    let command_config = config.clone();
    let dispatcher = Arc::new(dispatcher);
    channels
        .into_iter()
        .map(|channel| {
            let router = router.clone();
            let context = Arc::new(DaemonCommandContext::new(
                command_config.clone(),
                channel.id.clone(),
            ));
            let dispatcher = dispatcher.clone();
            tokio::spawn(async move {
                poll_channel(channel, router, context, dispatcher).await;
            })
        })
        .collect()
}

async fn poll_channel(
    channel: CommandChannel,
    router: Arc<crate::commands::CommandRouter>,
    context: Arc<DaemonCommandContext>,
    dispatcher: Arc<NotificationDispatcher>,
) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(40))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(channel = %channel.id, %error, "telegram command client failed to initialize");
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
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };

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
                continue;
            }
            let Some(text) =
                allowlisted_text(&channel.chat_id, message.chat.id, message.text.as_deref())
            else {
                continue;
            };
            let reply = router.dispatch(context.as_ref(), text).await;
            if dispatcher
                .send_telegram_reply(&channel.id, &reply)
                .await
                .is_err()
            {
                // Delivery errors can contain the token-bearing request URL.
                tracing::warn!(channel = %channel.id, "telegram command reply failed");
            }
        }
    }
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
}
