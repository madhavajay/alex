//! Small, daemon-local notification bus.  Channel configuration is supplied by
//! the `alex` binary, while the proxy owns dispatch because it knows when an
//! authenticated request has failed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationLevel {
    Info,
    Warn,
    Critical,
}

impl Default for NotificationLevel {
    fn default() -> Self {
        Self::Info
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationAccount {
    pub provider: String,
    /// Display-only email or local label. Never a credential.
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationEvent {
    pub level: NotificationLevel,
    pub category: String,
    pub title: String,
    pub body: String,
    pub account: NotificationAccount,
    #[serde(default)]
    pub action_url: Option<String>,
    /// Unix time in milliseconds.
    pub ts: i64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebhookFormat {
    #[default]
    Generic,
    Telegram,
    Slack,
    Discord,
}

impl WebhookFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Telegram => "telegram",
            Self::Slack => "slack",
            Self::Discord => "discord",
        }
    }
}

/// Persisted by the CLI's top-level `[[notifications]]` TOML entries. Every
/// field has a default so older config files and older config re-saves remain
/// compatible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationChannelConfig {
    /// Stable daemon-assigned identity used by the runtime admin API.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub format: WebhookFormat,
    #[serde(default)]
    pub url: String,
    /// Telegram bot token. It is intentionally never included in an admin
    /// view; Telegram delivery derives the sendMessage URL from this value.
    #[serde(default)]
    pub token: Option<String>,
    /// Cached display value returned by Telegram getMe; not a credential.
    #[serde(default)]
    pub bot_username: Option<String>,
    #[serde(default)]
    pub chat_id: Option<String>,
    /// Opt in to treating this Telegram chat as a daemon control channel.
    /// Defaults off so existing notification-only bots never accept commands.
    #[serde(default)]
    pub allow_commands: bool,
    #[serde(default)]
    pub min_level: NotificationLevel,
    #[serde(default)]
    pub categories: Vec<String>,
}

impl Default for NotificationChannelConfig {
    fn default() -> Self {
        Self {
            id: None,
            kind: default_kind(),
            format: WebhookFormat::default(),
            url: String::new(),
            token: None,
            bot_username: None,
            chat_id: None,
            allow_commands: false,
            min_level: NotificationLevel::default(),
            categories: Vec::new(),
        }
    }
}

fn default_kind() -> String {
    "webhook".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationSettings {
    #[serde(default)]
    pub channels: Vec<NotificationChannelConfig>,
    #[serde(default = "default_cooldown_seconds")]
    pub cooldown_seconds: u64,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            channels: Vec::new(),
            cooldown_seconds: default_cooldown_seconds(),
            timeout_seconds: default_timeout_seconds(),
        }
    }
}

pub fn default_cooldown_seconds() -> u64 {
    30 * 60
}

pub fn default_timeout_seconds() -> u64 {
    10
}

#[async_trait]
pub trait NotificationChannel: Send + Sync {
    fn kind(&self) -> &str;
    fn supports_replies(&self) -> bool {
        false
    }
    async fn send(&self, ev: &NotificationEvent) -> Result<()>;
    async fn send_text(&self, _text: &str) -> Result<()> {
        Err(anyhow!("notification channel does not support text replies"))
    }
}

#[derive(Clone)]
pub struct WebhookChannel {
    url: String,
    format: WebhookFormat,
    chat_id: Option<String>,
    client: reqwest::Client,
}

impl WebhookChannel {
    pub fn new(config: &NotificationChannelConfig) -> Result<Self> {
        let url = config.delivery_url();
        if url.trim().is_empty() {
            return Err(anyhow!("webhook URL is empty"));
        }
        if matches!(config.format, WebhookFormat::Telegram)
            && config.chat_id.as_deref().unwrap_or(" ").trim().is_empty()
        {
            return Err(anyhow!("telegram webhook requires chat_id"));
        }
        Ok(Self {
            url,
            format: config.format,
            chat_id: config.chat_id.clone(),
            client: reqwest::Client::new(),
        })
    }

    pub fn payload(&self, event: &NotificationEvent) -> Value {
        payload_for(self.format, self.chat_id.as_deref(), event)
    }
}

impl NotificationChannelConfig {
    /// Resolve the delivery URL at send time so Telegram tokens need not be
    /// duplicated into the persisted URL field.
    pub fn delivery_url(&self) -> String {
        if matches!(self.format, WebhookFormat::Telegram) {
            if let Some(token) = self
                .token
                .as_deref()
                .filter(|token| !token.trim().is_empty())
            {
                return telegram_send_message_url(token);
            }
        }
        self.url.clone()
    }
}

pub fn telegram_send_message_url(token: &str) -> String {
    format!("https://api.telegram.org/bot{token}/sendMessage")
}

pub fn telegram_get_updates_url(token: &str) -> String {
    format!("https://api.telegram.org/bot{token}/getUpdates")
}

#[async_trait]
impl NotificationChannel for WebhookChannel {
    fn kind(&self) -> &str {
        "webhook"
    }

    fn supports_replies(&self) -> bool {
        matches!(self.format, WebhookFormat::Telegram)
    }

    async fn send(&self, ev: &NotificationEvent) -> Result<()> {
        self.client
            .post(&self.url)
            .json(&self.payload(ev))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    async fn send_text(&self, text: &str) -> Result<()> {
        if !matches!(self.format, WebhookFormat::Telegram) {
            return Err(anyhow!("notification channel is not Telegram"));
        }
        self.client
            .post(&self.url)
            .json(&json!({
                "chat_id": self.chat_id.as_deref().unwrap_or_default(),
                "text": text,
            }))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}

pub fn render_event(event: &NotificationEvent) -> String {
    let account = event
        .account
        .label
        .as_deref()
        .map(|label| format!(" ({label})"))
        .unwrap_or_default();
    let action = event
        .action_url
        .as_deref()
        .map(|value| format!("\n\nAction: {value}"))
        .unwrap_or_default();
    format!(
        "{}\n{}{}{}",
        event.title,
        event.body,
        format!("\nAccount: {}{account}", event.account.provider),
        action
    )
}

pub fn payload_for(
    format: WebhookFormat,
    chat_id: Option<&str>,
    event: &NotificationEvent,
) -> Value {
    match format {
        WebhookFormat::Generic => serde_json::to_value(event).unwrap_or(Value::Null),
        WebhookFormat::Telegram => json!({
            "chat_id": chat_id.unwrap_or_default(),
            "text": render_event(event),
        }),
        WebhookFormat::Slack => json!({"text": render_event(event)}),
        WebhookFormat::Discord => json!({"content": render_event(event)}),
    }
}

#[derive(Clone)]
struct ChannelEntry {
    config: NotificationChannelConfig,
    channel: Arc<dyn NotificationChannel>,
}

#[derive(Debug, Clone, Default)]
struct ChannelStatus {
    last_sent_ms: Option<i64>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomDelivery {
    pub scheduled: bool,
    /// Command-enabled Telegram channel IDs that received this message.
    pub command_channel_ids: Vec<String>,
}

/// Fan-out dispatcher. The caller only updates a small debounce map; all HTTP
/// work happens in detached tasks and is bounded by a per-channel timeout.
#[derive(Clone)]
pub struct NotificationDispatcher {
    channels: Arc<Vec<ChannelEntry>>,
    cooldown: Duration,
    timeout: Duration,
    sent: Arc<Mutex<HashMap<String, i64>>>,
    status: Arc<Mutex<Vec<ChannelStatus>>>,
}

impl Default for NotificationDispatcher {
    fn default() -> Self {
        Self::from_settings(NotificationSettings::default())
    }
}

impl NotificationDispatcher {
    pub fn from_settings(settings: NotificationSettings) -> Self {
        let mut entries = Vec::new();
        for (index, config) in settings.channels.iter().enumerate() {
            if config.kind != "webhook" {
                tracing::warn!(channel = index, kind = %config.kind, "notification channel kind is not supported");
                continue;
            }
            match WebhookChannel::new(config) {
                Ok(channel) => entries.push(ChannelEntry {
                    config: config.clone(),
                    channel: Arc::new(channel),
                }),
                // Do not include the URL or the underlying error here: it may
                // contain a bearer token embedded in the URL.
                Err(_) => tracing::warn!(
                    channel = index,
                    "notification channel is disabled by invalid non-secret settings"
                ),
            }
        }
        Self::from_entries(
            entries,
            Duration::from_secs(settings.cooldown_seconds),
            Duration::from_secs(settings.timeout_seconds.max(1)),
        )
    }

    fn from_entries(entries: Vec<ChannelEntry>, cooldown: Duration, timeout: Duration) -> Self {
        let count = entries.len();
        Self {
            channels: Arc::new(entries),
            cooldown,
            timeout,
            sent: Arc::new(Mutex::new(HashMap::new())),
            status: Arc::new(Mutex::new(vec![ChannelStatus::default(); count])),
        }
    }

    pub fn emit(&self, event: NotificationEvent) {
        if !self.should_emit(&event) {
            return;
        }
        self.spawn_send(event, None);
    }

    /// Send account-specific re-authentication text to enabled Telegram
    /// channels without touching the normal event debounce map. Device-flow
    /// URLs are single-use, so suppressing a newly-created login because an
    /// older alert has the same title would strand the user with a stale link.
    ///
    /// Returns whether at least one Telegram delivery was scheduled. Other
    /// channel formats deliberately keep using the fixed event vocabulary.
    pub fn send_custom(
        &self,
        title: impl Into<String>,
        body: impl Into<String>,
        account: NotificationAccount,
    ) -> bool {
        self.send_custom_scoped(title, body, account, None)
            .scheduled
    }

    /// Send custom re-authentication text to all Telegram channels, or one
    /// explicitly selected command channel. The returned IDs are safe runtime
    /// channel identities, never chat IDs or bot tokens.
    pub fn send_custom_scoped(
        &self,
        title: impl Into<String>,
        body: impl Into<String>,
        account: NotificationAccount,
        only_channel_id: Option<&str>,
    ) -> CustomDelivery {
        let event = NotificationEvent {
            level: NotificationLevel::Warn,
            category: "reauth".into(),
            title: title.into(),
            body: body.into(),
            account,
            action_url: None,
            ts: crate::now_ms(),
        };
        let mut delivery = CustomDelivery::default();
        for (index, entry) in self.channels.iter().enumerate() {
            if !matches!(entry.config.format, WebhookFormat::Telegram)
                || only_channel_id.is_some_and(|wanted| {
                    entry.config.id.as_deref() != Some(wanted) || !entry.config.allow_commands
                })
                || !accepts(&entry.config, &event)
            {
                continue;
            }
            delivery.scheduled = true;
            if entry.config.allow_commands {
                if let Some(id) = entry.config.id.clone() {
                    delivery.command_channel_ids.push(id);
                }
            }
            self.spawn_send(event.clone(), Some(index));
        }
        delivery
    }

    pub fn has_enabled_telegram(&self) -> bool {
        self.channels.iter().any(|entry| {
            matches!(entry.config.format, WebhookFormat::Telegram)
                && NotificationLevel::Warn >= entry.config.min_level
                && (entry.config.categories.is_empty()
                    || entry
                        .config
                        .categories
                        .iter()
                        .any(|category| category == "reauth"))
        })
    }

    pub fn has_enabled_command_telegram(&self) -> bool {
        self.channels.iter().any(|entry| {
            matches!(entry.config.format, WebhookFormat::Telegram)
                && entry.config.allow_commands
                && NotificationLevel::Warn >= entry.config.min_level
                && (entry.config.categories.is_empty()
                    || entry
                        .config
                        .categories
                        .iter()
                        .any(|category| category == "reauth"))
        })
    }

    pub fn has_command_channel(&self, channel_id: &str) -> bool {
        self.channels.iter().any(|entry| {
            matches!(entry.config.format, WebhookFormat::Telegram)
                && entry.config.allow_commands
                && entry.config.id.as_deref() == Some(channel_id)
        })
    }

    /// Reply to one inbound Telegram control channel through the same
    /// configured sendMessage transport used by outbound notifications.
    pub async fn send_telegram_reply(&self, channel_id: &str, text: &str) -> Result<()> {
        let Some(entry) = self.channels.iter().find(|entry| {
            matches!(entry.config.format, WebhookFormat::Telegram)
                && entry.config.allow_commands
                && entry.config.id.as_deref() == Some(channel_id)
        }) else {
            return Err(anyhow!("telegram command channel is unavailable"));
        };
        match tokio::time::timeout(self.timeout, entry.channel.send_text(text)).await {
            Ok(Ok(())) => Ok(()),
            // The underlying HTTP error may contain the token-bearing URL.
            Ok(Err(_)) => Err(anyhow!("telegram reply delivery failed")),
            Err(_) => Err(anyhow!("telegram reply timed out")),
        }
    }

    pub fn emit_test(&self, channel: Option<usize>, now_ms: i64) {
        let dispatcher = self.clone();
        tokio::spawn(async move {
            let _ = dispatcher.test(channel, now_ms).await;
        });
    }

    /// Send a synthetic event and wait for each selected delivery. Admin test
    /// requests use this instead of the detached normal dispatch path so they
    /// can report an honest per-channel result. Tests intentionally bypass
    /// min_level/category filters.
    pub async fn test(&self, only: Option<usize>, now_ms: i64) -> Vec<Value> {
        let event = NotificationEvent {
            level: NotificationLevel::Info,
            category: "test".into(),
            title: "Alexandria notification test".into(),
            body: "This is a synthetic notification test event.".into(),
            account: NotificationAccount {
                provider: "alexandria".into(),
                label: None,
            },
            action_url: None,
            ts: now_ms,
        };
        let mut results = Vec::new();
        for (index, entry) in self.channels.iter().enumerate() {
            if only.is_some_and(|wanted| wanted != index) {
                continue;
            }
            let outcome = tokio::time::timeout(self.timeout, entry.channel.send(&event)).await;
            let (ok, error) = match outcome {
                Ok(Ok(())) => (true, None),
                // reqwest errors can contain a token-bearing URL. Keep the
                // externally visible result as deliberately generic as well.
                Ok(Err(_)) => (false, Some("delivery failed")),
                Err(_) => (false, Some("delivery timed out")),
            };
            if let Ok(mut statuses) = self.status.lock() {
                if let Some(status) = statuses.get_mut(index) {
                    if ok {
                        status.last_sent_ms = Some(event.ts);
                        status.last_error = None;
                    } else {
                        status.last_error = error.map(str::to_owned);
                    }
                }
            }
            results.push(json!({
                "index": index,
                "id": entry.config.id,
                "ok": ok,
                "error": error,
            }));
        }
        results
    }

    fn should_emit(&self, event: &NotificationEvent) -> bool {
        let key = format!(
            "{}:{}:{}:{}",
            event.category,
            event.account.provider,
            event.account.label.as_deref().unwrap_or(""),
            event.title
        );
        let now = event.ts;
        let mut sent = self
            .sent
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(previous) = sent.get(&key) {
            if now.saturating_sub(*previous) < self.cooldown.as_millis() as i64 {
                return false;
            }
        }
        sent.insert(key, now);
        true
    }

    fn spawn_send(&self, event: NotificationEvent, only: Option<usize>) {
        for (index, entry) in self.channels.iter().enumerate() {
            if only.is_some_and(|wanted| wanted != index) || !accepts(&entry.config, &event) {
                continue;
            }
            let channel = entry.channel.clone();
            let status = self.status.clone();
            let timeout = self.timeout;
            let event = event.clone();
            tokio::spawn(async move {
                let result = tokio::time::timeout(timeout, channel.send(&event)).await;
                let mut statuses = status
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let state = &mut statuses[index];
                match result {
                    Ok(Ok(())) => {
                        state.last_sent_ms = Some(event.ts);
                        state.last_error = None;
                    }
                    // Deliberately generic: reqwest errors often include the
                    // full request URL, which may itself be a secret.
                    Ok(Err(_)) => state.last_error = Some("delivery failed".into()),
                    Err(_) => state.last_error = Some("delivery timed out".into()),
                }
            });
        }
    }

    pub fn admin_view(&self) -> Value {
        let statuses = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let channels: Vec<Value> = self
            .channels
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let status = statuses.get(index).cloned().unwrap_or_default();
                json!({
                    "index": index,
                    "id": entry.config.id,
                    "kind": entry.channel.kind(),
                    "format": entry.config.format.as_str(),
                    "host": redacted_host(&entry.config.delivery_url()),
                    "bot_username": entry.config.bot_username,
                    "chat_id": entry.config.chat_id,
                    "allow_commands": entry.config.allow_commands,
                    "supports_replies": entry.channel.supports_replies(),
                    "min_level": entry.config.min_level,
                    "categories": entry.config.categories,
                    "last_sent": status.last_sent_ms,
                    "last_sent_ms": status.last_sent_ms,
                    "last_error": status.last_error,
                })
            })
            .collect();
        json!({"channels": channels, "cooldown_seconds": self.cooldown.as_secs(), "timeout_seconds": self.timeout.as_secs()})
    }
}

fn accepts(config: &NotificationChannelConfig, event: &NotificationEvent) -> bool {
    event.level >= config.min_level
        && (config.categories.is_empty()
            || config
                .categories
                .iter()
                .any(|category| category == &event.category))
}

fn redacted_host(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(String::from))
        .unwrap_or_else(|| "invalid".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn event() -> NotificationEvent {
        NotificationEvent {
            level: NotificationLevel::Warn,
            category: "reauth".into(),
            title: "Re-authentication needed".into(),
            body: "The token was rejected.".into(),
            account: NotificationAccount {
                provider: "openai".into(),
                label: Some("alex@example.test".into()),
            },
            action_url: Some("alex auth login openai".into()),
            ts: 1_000,
        }
    }

    #[test]
    fn payloads_have_the_expected_minimal_shapes() {
        let event = event();
        assert_eq!(
            payload_for(WebhookFormat::Generic, None, &event)["category"],
            "reauth"
        );
        let telegram = payload_for(WebhookFormat::Telegram, Some("123"), &event);
        assert_eq!(telegram["chat_id"], "123");
        assert!(telegram["text"]
            .as_str()
            .unwrap()
            .contains("Re-authentication"));
        assert_eq!(
            payload_for(WebhookFormat::Slack, None, &event)["text"].is_string(),
            true
        );
        assert_eq!(
            payload_for(WebhookFormat::Discord, None, &event)["content"].is_string(),
            true
        );
    }

    #[test]
    fn serialized_event_never_contains_webhook_token() {
        let token_url = "https://api.telegram.org/botTOP_SECRET/sendMessage";
        let encoded = serde_json::to_string(&event()).unwrap();
        assert!(!encoded.contains("TOP_SECRET"));
        assert!(
            !NotificationDispatcher::from_settings(NotificationSettings {
                channels: vec![NotificationChannelConfig {
                    url: token_url.into(),
                    ..Default::default()
                }],
                ..Default::default()
            })
            .admin_view()
            .to_string()
            .contains("TOP_SECRET")
        );
    }

    #[test]
    fn telegram_token_derives_send_message_url() {
        let channel = NotificationChannelConfig {
            format: WebhookFormat::Telegram,
            token: Some("123:secret".into()),
            ..Default::default()
        };
        assert_eq!(
            channel.delivery_url(),
            "https://api.telegram.org/bot123:secret/sendMessage"
        );
        assert!(
            WebhookChannel::new(&channel).is_err(),
            "chat_id is required"
        );
        assert!(!channel.allow_commands, "inbound commands default off");
    }

    struct CountingChannel(AtomicUsize);

    #[async_trait]
    impl NotificationChannel for CountingChannel {
        fn kind(&self) -> &str {
            "test"
        }
        async fn send(&self, _: &NotificationEvent) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn coalesces_identical_auth_events() {
        let channel = Arc::new(CountingChannel(AtomicUsize::new(0)));
        let dispatcher = NotificationDispatcher::from_entries(
            vec![ChannelEntry {
                config: NotificationChannelConfig::default(),
                channel: channel.clone(),
            }],
            Duration::from_secs(30 * 60),
            Duration::from_secs(1),
        );
        dispatcher.emit(event());
        dispatcher.emit(event());
        tokio::task::yield_now().await;
        assert_eq!(channel.0.load(Ordering::SeqCst), 1);
    }

    struct RecordingChannel(Mutex<Vec<NotificationEvent>>);

    #[async_trait]
    impl NotificationChannel for RecordingChannel {
        fn kind(&self) -> &str {
            "test"
        }

        async fn send(&self, event: &NotificationEvent) -> Result<()> {
            self.0.lock().unwrap().push(event.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn custom_telegram_text_is_never_debounced() {
        let channel = Arc::new(RecordingChannel(Mutex::new(Vec::new())));
        let dispatcher = NotificationDispatcher::from_entries(
            vec![ChannelEntry {
                config: NotificationChannelConfig {
                    format: WebhookFormat::Telegram,
                    chat_id: Some("123".into()),
                    categories: vec!["reauth".into()],
                    ..Default::default()
                },
                channel: channel.clone(),
            }],
            Duration::from_secs(30 * 60),
            Duration::from_secs(1),
        );
        let account = NotificationAccount {
            provider: "xai".into(),
            label: Some("work".into()),
        };
        let first_verification_uri_complete =
            "https://auth.example.test/device?code=first";
        assert!(dispatcher.send_custom(
            "Grok needs re-authentication",
            format!("Tap {first_verification_uri_complete}"),
            account.clone(),
        ));
        assert!(dispatcher.send_custom(
            "Grok needs re-authentication",
            "Tap https://auth.example.test/device?code=second",
            account,
        ));
        tokio::task::yield_now().await;

        let events = channel.0.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(render_event(&events[0]).contains(first_verification_uri_complete));
        assert!(events[1].body.contains("code=second"));
    }

    struct TextChannel(Mutex<Vec<String>>);

    #[async_trait]
    impl NotificationChannel for TextChannel {
        fn kind(&self) -> &str {
            "test"
        }

        async fn send(&self, _: &NotificationEvent) -> Result<()> {
            Ok(())
        }

        async fn send_text(&self, text: &str) -> Result<()> {
            self.0.lock().unwrap().push(text.to_string());
            Ok(())
        }
    }

    #[test]
    fn paste_reauth_requires_an_opted_in_command_telegram() {
        let mut channel = NotificationChannelConfig {
            format: WebhookFormat::Telegram,
            url: "https://telegram.example.test/send".into(),
            chat_id: Some("123".into()),
            categories: vec!["reauth".into()],
            ..Default::default()
        };
        let notification_only = NotificationDispatcher::from_settings(NotificationSettings {
            channels: vec![channel.clone()],
            ..Default::default()
        });
        assert!(notification_only.has_enabled_telegram());
        assert!(!notification_only.has_enabled_command_telegram());

        channel.allow_commands = true;
        let command_enabled = NotificationDispatcher::from_settings(NotificationSettings {
            channels: vec![channel],
            ..Default::default()
        });
        assert!(command_enabled.has_enabled_command_telegram());
    }

    #[tokio::test]
    async fn telegram_reply_uses_only_an_opted_in_control_channel() {
        let channel = Arc::new(TextChannel(Mutex::new(Vec::new())));
        let dispatcher = NotificationDispatcher::from_entries(
            vec![ChannelEntry {
                config: NotificationChannelConfig {
                    id: Some("control".into()),
                    format: WebhookFormat::Telegram,
                    chat_id: Some("123".into()),
                    allow_commands: true,
                    ..Default::default()
                },
                channel: channel.clone(),
            }],
            Duration::from_secs(1),
            Duration::from_secs(1),
        );

        dispatcher
            .send_telegram_reply("control", "pong")
            .await
            .unwrap();
        assert_eq!(*channel.0.lock().unwrap(), ["pong"]);
        assert!(dispatcher.send_telegram_reply("foreign", "nope").await.is_err());
    }
}
