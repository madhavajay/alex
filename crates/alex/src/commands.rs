//! Transport-independent inbound command parsing and dispatch.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use reqwest::Method;

use crate::status::{status_summary, StatusSummary};
use crate::{daemon_send, now_ms, Config};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedCommand {
    pub name: Option<String>,
    pub args: Vec<String>,
}

impl ParsedCommand {
    pub(crate) fn parse(text: &str) -> Self {
        let mut words = text.split_whitespace();
        let Some(first) = words.next() else {
            return Self {
                name: None,
                args: Vec::new(),
            };
        };
        let name = first
            .strip_prefix('/')
            .filter(|name| !name.is_empty())
            .map(|name| name.split('@').next().unwrap_or(name).to_ascii_lowercase());
        Self {
            name,
            args: words.map(str::to_owned).collect(),
        }
    }
}

#[async_trait]
pub(crate) trait CommandContext: Send + Sync {
    async fn status(&self) -> Result<StatusSummary>;
    async fn start_reauth(&self, provider: &str) -> Result<ReauthFlow>;
    /// `None` means this channel has no pending paste-mode session.
    async fn submit_code(&self, input: &str) -> Result<Option<CodeSubmission>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReauthFlow {
    pub provider: String,
    pub mode: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeSubmission {
    pub provider: Option<String>,
    pub ok: bool,
    pub expired: bool,
    pub error: Option<String>,
}

#[derive(Clone)]
pub(crate) struct DaemonCommandContext {
    config: Config,
    channel_id: String,
}

impl DaemonCommandContext {
    pub(crate) fn new(config: Config, channel_id: String) -> Self {
        Self { config, channel_id }
    }
}

#[async_trait]
impl CommandContext for DaemonCommandContext {
    async fn status(&self) -> Result<StatusSummary> {
        status_summary(&self.config).await
    }

    async fn start_reauth(&self, provider: &str) -> Result<ReauthFlow> {
        let (status, body) = daemon_send(
            &self.config,
            Method::POST,
            "/admin/auth/reauth-notify",
            Some(serde_json::json!({
                "provider": provider,
                "channel_id": self.channel_id,
            })),
        )
        .await?;
        if !status.is_success() {
            anyhow::bail!("could not start re-authentication for {provider}");
        }
        let mode = body["mode"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("reauth response omitted session mode"))?;
        let url = body["verification_uri_complete"]
            .as_str()
            .filter(|url| !url.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("reauth response omitted authorization link"))?;
        Ok(ReauthFlow {
            provider: body["provider"].as_str().unwrap_or(provider).to_string(),
            mode: mode.to_string(),
            url: url.to_string(),
        })
    }

    async fn submit_code(&self, input: &str) -> Result<Option<CodeSubmission>> {
        let (status, body) = daemon_send(
            &self.config,
            Method::POST,
            "/admin/auth/reauth/code",
            Some(serde_json::json!({
                "channel_id": self.channel_id,
                "input": input,
            })),
        )
        .await?;
        if !status.is_success() {
            anyhow::bail!("code submission was rejected");
        }
        let awaiting = body["awaiting"].as_bool().unwrap_or(false);
        let expired = body["expired"].as_bool().unwrap_or(false);
        if !awaiting && !expired && body["ok"].as_bool() != Some(true) {
            return Ok(None);
        }
        Ok(Some(CodeSubmission {
            provider: body["provider"].as_str().map(str::to_owned),
            ok: body["ok"].as_bool().unwrap_or(false),
            expired,
            error: body["error"].as_str().map(str::to_owned),
        }))
    }
}

#[async_trait]
trait CommandHandler: Send + Sync {
    async fn handle(
        &self,
        context: &dyn CommandContext,
        args: &[String],
        commands: &[CommandHelp],
    ) -> String;
}

#[derive(Debug, Clone)]
struct CommandHelp {
    name: &'static str,
    description: &'static str,
}

struct RegisteredCommand {
    help: CommandHelp,
    handler: Arc<dyn CommandHandler>,
}

/// A table-driven router with no Telegram concepts. Adding a command is one
/// `register` call in `daemon_command_router`, plus its handler implementation.
pub(crate) struct CommandRouter {
    commands: BTreeMap<&'static str, RegisteredCommand>,
    fallback: Arc<dyn CommandHandler>,
}

impl CommandRouter {
    fn new(fallback: impl CommandHandler + 'static) -> Self {
        Self {
            commands: BTreeMap::new(),
            fallback: Arc::new(fallback),
        }
    }

    fn register(
        mut self,
        name: &'static str,
        description: &'static str,
        handler: impl CommandHandler + 'static,
    ) -> Self {
        self.commands.insert(
            name,
            RegisteredCommand {
                help: CommandHelp { name, description },
                handler: Arc::new(handler),
            },
        );
        self
    }

    pub(crate) async fn dispatch(&self, context: &dyn CommandContext, text: &str) -> String {
        let parsed = ParsedCommand::parse(text);
        let help: Vec<CommandHelp> = self
            .commands
            .values()
            .map(|command| command.help.clone())
            .collect();
        if parsed.name.is_none() {
            match context.submit_code(text.trim()).await {
                Ok(Some(result)) => return code_submission_reply(result),
                Ok(None) => {}
                Err(_) => return "Could not submit the code; try again shortly.".to_string(),
            }
        }
        let handler = parsed
            .name
            .as_deref()
            .and_then(|name| self.commands.get(name))
            .map(|command| command.handler.as_ref())
            .unwrap_or(self.fallback.as_ref());
        handler.handle(context, &parsed.args, &help).await
    }
}

pub(crate) fn daemon_command_router() -> CommandRouter {
    CommandRouter::new(AiFallbackHandler)
        .register("code", "submit an awaited OAuth code#state", CodeHandler)
        .register("help", "list available commands", HelpHandler)
        .register("ping", "check daemon liveness", PingHandler)
        .register("reauth", "start provider re-authentication", ReauthHandler)
        .register("status", "show daemon and account status", StatusHandler)
        .register("usage", "coming soon", ComingSoonHandler)
}

struct CodeHandler;

#[async_trait]
impl CommandHandler for CodeHandler {
    async fn handle(
        &self,
        context: &dyn CommandContext,
        args: &[String],
        _commands: &[CommandHelp],
    ) -> String {
        if args.is_empty() {
            return "Usage: /code <code#state>".to_string();
        }
        match context.submit_code(&args.join(" ")).await {
            Ok(Some(result)) => code_submission_reply(result),
            Ok(None) => "No paste-code re-authentication is awaiting a code.".to_string(),
            Err(_) => "Could not submit the code; try again shortly.".to_string(),
        }
    }
}

fn code_submission_reply(result: CodeSubmission) -> String {
    if result.ok {
        return format!(
            "✅ {} re-authenticated",
            result.provider.as_deref().unwrap_or("provider")
        );
    }
    if result.expired {
        return "The re-authentication session expired; run /reauth <provider> again.".to_string();
    }
    result.error.unwrap_or_else(|| {
        "OAuth exchange failed; paste a fresh code#state and try again".to_string()
    })
}

struct ReauthHandler;

#[async_trait]
impl CommandHandler for ReauthHandler {
    async fn handle(
        &self,
        context: &dyn CommandContext,
        args: &[String],
        _commands: &[CommandHelp],
    ) -> String {
        let Some(provider) = args.first() else {
            return providers_needing_reauth(context).await;
        };
        match context.start_reauth(provider).await {
            Ok(flow) => reauth_flow_reply(&flow),
            Err(_) => format!("Could not start re-authentication for {provider}."),
        }
    }
}

fn reauth_flow_reply(flow: &ReauthFlow) -> String {
    if flow.mode == "paste" {
        format!(
            "Re-authenticate {}:\n{}\n\nAfter approving, paste the code#state here.",
            flow.provider, flow.url
        )
    } else {
        format!(
            "Re-authenticate {}:\n{}\n\nAlexandria is waiting for authorization and will finish automatically.",
            flow.provider, flow.url
        )
    }
}

async fn providers_needing_reauth(context: &dyn CommandContext) -> String {
    let Ok(summary) = context.status().await else {
        return "Could not read account status; try /reauth <provider>.".to_string();
    };
    let mut providers = std::collections::BTreeSet::new();
    for account in summary.accounts {
        if account.needs_reauth
            || account
                .expires_at_ms
                .is_some_and(|expiry| expiry <= now_ms())
        {
            providers.insert(account.provider);
        }
    }
    if providers.is_empty() {
        return "No providers currently need re-authentication.".to_string();
    }
    let mut lines = vec!["Providers needing re-authentication:".to_string()];
    lines.extend(
        providers
            .into_iter()
            .map(|provider| format!("• {provider} — /reauth {provider}")),
    );
    lines.join("\n")
}

struct StatusHandler;

#[async_trait]
impl CommandHandler for StatusHandler {
    async fn handle(
        &self,
        context: &dyn CommandContext,
        _args: &[String],
        _commands: &[CommandHelp],
    ) -> String {
        match context.status().await {
            Ok(summary) => summary.telegram_text(),
            Err(_) => "status unavailable; try again shortly".to_string(),
        }
    }
}

struct PingHandler;

#[async_trait]
impl CommandHandler for PingHandler {
    async fn handle(
        &self,
        context: &dyn CommandContext,
        _args: &[String],
        _commands: &[CommandHelp],
    ) -> String {
        match context.status().await {
            Ok(summary) => summary.ping_text(),
            Err(_) => format!(
                "pong · Alexandria v{} · uptime unknown",
                env!("CARGO_PKG_VERSION")
            ),
        }
    }
}

struct HelpHandler;

#[async_trait]
impl CommandHandler for HelpHandler {
    async fn handle(
        &self,
        _context: &dyn CommandContext,
        _args: &[String],
        commands: &[CommandHelp],
    ) -> String {
        let mut lines = vec!["Alexandria commands:".to_string()];
        lines.extend(
            commands
                .iter()
                .map(|command| format!("/{} — {}", command.name, command.description)),
        );
        lines.join("\n")
    }
}

struct ComingSoonHandler;

#[async_trait]
impl CommandHandler for ComingSoonHandler {
    async fn handle(
        &self,
        _context: &dyn CommandContext,
        _args: &[String],
        _commands: &[CommandHelp],
    ) -> String {
        "coming soon".to_string()
    }
}

struct AiFallbackHandler;

#[async_trait]
impl CommandHandler for AiFallbackHandler {
    async fn handle(
        &self,
        _context: &dyn CommandContext,
        _args: &[String],
        _commands: &[CommandHelp],
    ) -> String {
        // TODO(ai-command-fallback): pass unmatched slash commands and free
        // text to a read-only AI handler once its trust boundary is defined.
        "unknown command; try /help".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::StatusAccount;
    use crate::ServiceState;

    struct SyntheticContext;

    #[async_trait]
    impl CommandContext for SyntheticContext {
        async fn status(&self) -> Result<StatusSummary> {
            Ok(StatusSummary {
                version: "1.2.3".into(),
                update_available: false,
                update_target: None,
                daemon_up: true,
                uptime_s: Some(90),
                service: "active".into(),
                service_managed: true,
                dario_ready: false,
                accounts: vec![StatusAccount {
                    id: "openai-oauth".into(),
                    provider: "openai".into(),
                    name: "default".into(),
                    kind: "oauth".into(),
                    label: None,
                    status: "active".into(),
                    health: "healthy".into(),
                    needs_reauth: false,
                    usage_pct: None,
                    expires_at_ms: None,
                    last_heartbeat: serde_json::Value::Null,
                }],
                binaries: Vec::new(),
                base_url: "http://127.0.0.1:4100".into(),
                health: None,
                limits: None,
                dario: None,
                dario_response: None,
                update: None,
                admin_health: None,
                service_state: ServiceState::Unsupported,
            })
        }

        async fn start_reauth(&self, _provider: &str) -> Result<ReauthFlow> {
            anyhow::bail!("not configured")
        }

        async fn submit_code(&self, _input: &str) -> Result<Option<CodeSubmission>> {
            Ok(None)
        }
    }

    struct ReauthContext {
        awaiting: std::sync::atomic::AtomicBool,
        submitted: std::sync::Mutex<Vec<String>>,
        started: std::sync::Mutex<Vec<String>>,
    }

    impl ReauthContext {
        fn awaiting() -> Self {
            Self {
                awaiting: std::sync::atomic::AtomicBool::new(true),
                submitted: std::sync::Mutex::new(Vec::new()),
                started: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl CommandContext for ReauthContext {
        async fn status(&self) -> Result<StatusSummary> {
            SyntheticContext.status().await
        }

        async fn start_reauth(&self, provider: &str) -> Result<ReauthFlow> {
            self.started.lock().unwrap().push(provider.to_string());
            Ok(ReauthFlow {
                provider: provider.to_string(),
                mode: "paste".into(),
                url: "https://auth.example/authorize".into(),
            })
        }

        async fn submit_code(&self, input: &str) -> Result<Option<CodeSubmission>> {
            if !self
                .awaiting
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Ok(None);
            }
            self.submitted.lock().unwrap().push(input.to_string());
            Ok(Some(CodeSubmission {
                provider: Some("anthropic".into()),
                ok: true,
                expired: false,
                error: None,
            }))
        }
    }

    #[tokio::test]
    async fn parses_and_dispatches_registered_and_unknown_commands() {
        assert_eq!(
            ParsedCommand::parse(" /STATUS@AlexBot extra "),
            ParsedCommand {
                name: Some("status".into()),
                args: vec!["extra".into()],
            }
        );

        let router = daemon_command_router();
        let context = SyntheticContext;
        assert!(router
            .dispatch(&context, "/status")
            .await
            .contains("Accounts:"));
        assert!(router.dispatch(&context, "/help").await.contains("/ping"));
        assert_eq!(
            router.dispatch(&context, "/ping").await,
            "pong · Alexandria v1.2.3 · uptime 1m"
        );
        assert_eq!(
            router.dispatch(&context, "/does-not-exist").await,
            "unknown command; try /help"
        );
        assert_eq!(
            router.dispatch(&context, "hello alex").await,
            "unknown command; try /help"
        );
    }

    #[tokio::test]
    async fn awaited_plain_text_and_explicit_code_route_to_completion() {
        let router = daemon_command_router();
        let plain = ReauthContext::awaiting();
        assert_eq!(
            router.dispatch(&plain, "paste-code#state").await,
            "✅ anthropic re-authenticated"
        );
        assert_eq!(*plain.submitted.lock().unwrap(), ["paste-code#state"]);

        let explicit = ReauthContext::awaiting();
        assert_eq!(
            router.dispatch(&explicit, "/code other-code#state").await,
            "✅ anthropic re-authenticated"
        );
        assert_eq!(*explicit.submitted.lock().unwrap(), ["other-code#state"]);
    }

    #[tokio::test]
    async fn reauth_provider_starts_a_flow_and_returns_paste_instructions() {
        let router = daemon_command_router();
        let context = ReauthContext::awaiting();
        let reply = router.dispatch(&context, "/reauth anthropic").await;
        assert!(reply.contains("https://auth.example/authorize"));
        assert!(reply.contains("paste the code#state here"));
        assert_eq!(*context.started.lock().unwrap(), ["anthropic"]);
    }
}
