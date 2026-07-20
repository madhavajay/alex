use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::str::FromStr;
use std::time::Duration;

use alex_core::{
    build_resume_context_from_captures, ClientFormat, ResumeCapture, ResumeContext, ResumeEntry,
};
use alex_store::{SessionForkRecord, Store, TraceFilter};
use anyhow::{bail, Context, Result};
use chrono::{Local, SecondsFormat, Utc};
use flate2::read::GzDecoder;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};
use toml_edit::DocumentMut;
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::{harness_connect, now_ms, ui, Config, RawModeGuard};

const RESUME_CONTEXT_MAX_CHARS: usize = 200_000;
// Linux rejects a single exec argument around 128 KiB even when ARG_MAX is
// larger. Keep enough headroom for the harness flags and multibyte text.
const RESUME_PROMPT_MAX_BYTES: usize = 96 * 1024;
const RESUME_HARNESSES: &[&str] = &["pi", "claude", "codex"];
const FORK_DISCOVERY_LIMIT: usize = 100;
const PI_SESSION_VERSION: u64 = 3;

#[derive(Debug)]
struct CapturedExchange {
    client_format: ClientFormat,
    request: Value,
    response_format: ClientFormat,
    response: String,
}

#[derive(Debug)]
struct ResumeSource {
    session_id: String,
    harness: String,
    captures: Vec<CapturedExchange>,
    requested_model: Option<String>,
    routed_model: Option<String>,
    trace_count: usize,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectoryResolution {
    path: PathBuf,
    evidence: Option<PathBuf>,
    evidence_semantics: Option<&'static str>,
    fallback_reason: Option<String>,
}

#[derive(Debug)]
struct LaunchPlan {
    harness: String,
    binary: PathBuf,
    args: Vec<OsString>,
    cwd: PathBuf,
    config_dir: PathBuf,
    model: ModelSelection,
    mode: ResumeMode,
}

#[derive(Debug, Clone)]
struct ModelSelection {
    model: String,
    reason: Option<String>,
}

#[derive(Debug)]
enum ResumeMode {
    PromptPaste { reason: String },
    NativePi(PiSessionDraft),
    NativeClaude(ClaudeSessionDraft),
    NativeCodex(CodexSessionDraft),
}

#[derive(Debug)]
struct PiSessionDraft {
    id: String,
    path: PathBuf,
    jsonl: String,
}

#[derive(Debug)]
struct ClaudeSessionDraft {
    id: String,
    path: PathBuf,
    jsonl: String,
}

#[derive(Debug)]
struct CodexSessionDraft {
    id: String,
    path: PathBuf,
    jsonl: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClaudeSessionFormat {
    version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexSessionFormat {
    cli_version: String,
    history_mode: String,
}

pub(crate) async fn resume_cmd(
    config: &Config,
    session_id: &str,
    requested_harness: Option<&str>,
    source_harness: Option<&str>,
    requested_model: Option<&str>,
    paste: bool,
    dry_run: bool,
) -> Result<()> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        bail!("session id must not be empty");
    }

    let store = Store::open(config.data_dir.clone())?;
    let source = load_resume_source(&store, session_id, source_harness)?;
    let directory = recover_directory(config, &source)?;
    let target = resolve_target_harness(config, requested_harness).await?;
    let fork_token = Uuid::new_v4().to_string();
    let (context, prompt) = build_fork_context(&source, &fork_token);
    if context.included_entries == 0 {
        bail!(
            "session '{}' has no recoverable conversation content",
            source.session_id
        );
    }
    let plan = build_launch_plan(
        config,
        &target,
        &source,
        &context,
        &directory.path,
        &prompt,
        requested_model,
        paste,
    )?;

    print_resume_summary(&source, &context, &directory, &plan, dry_run);
    if dry_run {
        return Ok(());
    }

    launch_and_record_fork(&store, &source, &directory, plan, &fork_token).await
}

fn load_resume_source(
    store: &Store,
    session_id: &str,
    source_harness: Option<&str>,
) -> Result<ResumeSource> {
    let mut rows = store.session_traces(session_id, None)?;
    if rows.is_empty() {
        bail!("no captured session found for '{session_id}'");
    }

    let requested_source = source_harness.map(canonical_harness);
    if let Some(requested) = requested_source.as_deref() {
        rows.retain(|row| {
            row["harness"].as_str().map(canonical_harness).as_deref() == Some(requested)
        });
        if rows.is_empty() {
            bail!("session '{session_id}' has no traces from source harness '{requested}'");
        }
    }

    let harnesses: BTreeSet<String> = rows
        .iter()
        .filter_map(|row| row["harness"].as_str())
        .map(canonical_harness)
        .collect();
    if harnesses.len() > 1 {
        bail!(
            "session id '{session_id}' is shared by multiple harnesses ({}); retry with --source-harness <name>",
            harnesses.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    let harness = harnesses
        .into_iter()
        .next()
        .context("captured session does not identify its source harness")?;

    let mut captures = Vec::new();
    let mut unreadable_requests = 0usize;
    let mut bodyless_requests = 0usize;
    let mut unsupported_requests = 0usize;
    let mut missing_responses = 0usize;
    let mut last_request_error = None;
    for row in &rows {
        let Some(request_path) = row["req_body_path"].as_str() else {
            bodyless_requests += 1;
            continue;
        };
        let Some(client_format) = parse_client_format(row["client_format"].as_str()) else {
            unsupported_requests += 1;
            continue;
        };
        let Some(response_format) = parse_client_format(
            row["upstream_format"]
                .as_str()
                .or_else(|| row["client_format"].as_str()),
        ) else {
            unsupported_requests += 1;
            continue;
        };
        let request = read_gzip(request_path)
            .with_context(|| format!("reading request body at {request_path}"))
            .and_then(|bytes| {
                serde_json::from_slice::<Value>(&bytes)
                    .context("captured request body is not valid JSON")
            });
        match request {
            Ok(request) => {
                let response = match row["resp_body_path"].as_str() {
                    Some(path) => match read_gzip(path) {
                        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                        Err(_) => {
                            missing_responses += 1;
                            String::new()
                        }
                    },
                    None => {
                        missing_responses += 1;
                        String::new()
                    }
                };
                captures.push(CapturedExchange {
                    client_format,
                    request,
                    response_format,
                    response,
                });
            }
            Err(error) => {
                unreadable_requests += 1;
                last_request_error = Some(error);
            }
        }
    }
    if captures.is_empty() {
        let detail = last_request_error
            .map(|error| format!(": {error:#}"))
            .unwrap_or_default();
        bail!("captured session has no readable request body to resume{detail}");
    }

    let mut warnings = Vec::new();
    if unreadable_requests > 0 {
        warnings.push(format!(
            "{unreadable_requests} captured request bod{} unreadable and skipped",
            if unreadable_requests == 1 {
                "y was"
            } else {
                "ies were"
            }
        ));
    }
    if bodyless_requests > 0 {
        warnings.push(format!(
            "{bodyless_requests} trace{} had no retained request body and could not contribute context",
            if bodyless_requests == 1 { "" } else { "s" }
        ));
    }
    if unsupported_requests > 0 {
        warnings.push(format!(
            "{unsupported_requests} trace{} used an unsupported or unidentified format and was skipped",
            if unsupported_requests == 1 { "" } else { "s" }
        ));
    }
    if missing_responses > 0 {
        warnings.push(format!(
            "{missing_responses} capture{} had no readable completed response; request history was retained",
            if missing_responses == 1 { "" } else { "s" }
        ));
    }
    let routed_model = rows
        .iter()
        .rev()
        .find_map(|row| row["routed_model"].as_str().map(String::from));
    let requested_model = rows
        .iter()
        .rev()
        .find_map(|row| row["requested_model"].as_str().map(String::from));

    Ok(ResumeSource {
        session_id: session_id.to_string(),
        harness,
        captures,
        requested_model,
        routed_model,
        trace_count: rows.len(),
        warnings,
    })
}

fn read_gzip(path: &str) -> Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("opening {path}"))?;
    let mut decoder = GzDecoder::new(file);
    let mut bytes = Vec::new();
    decoder
        .read_to_end(&mut bytes)
        .with_context(|| format!("decompressing {path}"))?;
    Ok(bytes)
}

fn parse_client_format(value: Option<&str>) -> Option<ClientFormat> {
    match value? {
        "anthropic" => Some(ClientFormat::AnthropicMessages),
        "openai-chat" => Some(ClientFormat::OpenaiChat),
        "openai-responses" => Some(ClientFormat::OpenaiResponses),
        "gemini" => Some(ClientFormat::GeminiGenerate),
        _ => None,
    }
}

fn canonical_harness(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    if value.starts_with("claude") {
        "claude".into()
    } else if value.starts_with("codex") {
        "codex".into()
    } else if value == "pi" || value.starts_with("pi/") || value.starts_with("pi-") {
        "pi".into()
    } else {
        value
    }
}

fn fork_prompt(context: &ResumeContext, source: &ResumeSource, token: &str) -> String {
    let metadata = serde_json::json!({
        "type": "alex_fork_metadata",
        "source_session": source.session_id,
        "source_harness": source.harness,
        "token": token,
    });
    format!("{}\n\n{metadata}", context.prompt)
}

fn build_fork_context(source: &ResumeSource, token: &str) -> (ResumeContext, String) {
    let captures = source
        .captures
        .iter()
        .map(|capture| ResumeCapture {
            client_format: capture.client_format,
            request: &capture.request,
            response_format: capture.response_format,
            raw_response: &capture.response,
        })
        .collect::<Vec<_>>();
    let mut max_chars = RESUME_CONTEXT_MAX_CHARS;
    loop {
        let context = build_resume_context_from_captures(&source.session_id, &captures, max_chars);
        let prompt = fork_prompt(&context, source, token);
        if prompt.len() <= RESUME_PROMPT_MAX_BYTES || max_chars == 0 {
            return (context, prompt);
        }

        // Scale the Unicode-character cap by the observed UTF-8 size, with a
        // little slack for truncation-notice growth at entry boundaries.
        let scaled = max_chars
            .saturating_mul(RESUME_PROMPT_MAX_BYTES)
            .checked_div(prompt.len())
            .unwrap_or(0);
        max_chars = scaled.saturating_mul(9).checked_div(10).unwrap_or(0);
    }
}

async fn resolve_target_harness(config: &Config, requested: Option<&str>) -> Result<String> {
    let statuses = harness_connect::harness_statuses(config, None, true).await?;
    let candidates = resume_candidates(&statuses);
    if let Some(requested) = requested {
        let requested = canonical_harness(requested);
        if !RESUME_HARNESSES.contains(&requested.as_str()) {
            bail!(
                "harness '{requested}' cannot start an interactive fork yet; supported harnesses: {}",
                RESUME_HARNESSES.join(", ")
            );
        }
        let status = statuses
            .iter()
            .find(|status| status.name == requested)
            .with_context(|| format!("unknown harness '{requested}'"))?;
        if !status.installed {
            bail!("{requested} is not installed");
        }
        if !status.connected {
            bail!("{requested} is not connected to Alex; run `alex connect {requested}` first");
        }
        return Ok(requested);
    }

    if candidates.is_empty() {
        bail!(
            "no resume-capable harness is connected; run `alex connect pi`, `alex connect claude`, or `alex connect codex` first"
        );
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        bail!(
            "a harness is required outside an interactive terminal; usage: alex resume <session-id> <{}>",
            RESUME_HARNESSES.join("|")
        );
    }
    pick_harness(&candidates)?.context("resume cancelled")
}

fn resume_candidates(statuses: &[harness_connect::HarnessStatus]) -> Vec<String> {
    RESUME_HARNESSES
        .iter()
        .filter(|name| {
            statuses
                .iter()
                .any(|status| status.name == **name && status.installed && status.connected)
        })
        .map(|name| (*name).to_string())
        .collect()
}

fn pick_harness(harnesses: &[String]) -> Result<Option<String>> {
    use crossterm::cursor::{MoveToColumn, MoveUp};
    use crossterm::event::{read, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{Clear, ClearType};

    let mut out = std::io::stdout();
    writeln!(
        out,
        "{} {}",
        ui::gold(ui::diamond()),
        ui::bold("choose a connected harness for the fork")
    )?;
    let guard = RawModeGuard::new()?;
    let mut selected = 0usize;
    let mut drawn = false;
    let choice = loop {
        if drawn {
            crossterm::execute!(out, MoveUp(harnesses.len() as u16))?;
        }
        for (index, harness) in harnesses.iter().enumerate() {
            crossterm::execute!(out, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
            let marker = if index == selected {
                ui::gold(ui::selector())
            } else {
                " ".into()
            };
            let name = if index == selected {
                ui::bold(harness)
            } else {
                harness.clone()
            };
            write!(out, " {marker} {}\r\n", ui::pad_right(&name, 10))?;
        }
        out.flush()?;
        drawn = true;
        match read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = selected.checked_sub(1).unwrap_or(harnesses.len() - 1)
                }
                KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1) % harnesses.len(),
                KeyCode::Enter => break Some(harnesses[selected].clone()),
                KeyCode::Esc | KeyCode::Char('q') => break None,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break None,
                _ => {}
            },
            _ => {}
        }
    };
    drop(guard);
    Ok(choice)
}

fn build_launch_plan(
    config: &Config,
    target: &str,
    source: &ResumeSource,
    context: &ResumeContext,
    cwd: &Path,
    prompt: &str,
    requested_model: Option<&str>,
    paste: bool,
) -> Result<LaunchPlan> {
    let spec = harness_connect::spec_by_name(target)
        .with_context(|| format!("unknown harness '{target}'"))?;
    let config_dir = harness_connect::resolve_config_dir(config, spec, None);
    let binary = harness_connect::resolve_harness_binary(config, spec)
        .with_context(|| format!("{target} is not installed or not on PATH"))?;
    let model = select_resume_model(target, &config_dir, source, requested_model)?;
    let mut args = match target {
        "pi" => vec![
            OsString::from("--provider"),
            OsString::from("alexandria"),
            OsString::from("--model"),
            OsString::from(&model.model),
        ],
        "claude" => vec![
            OsString::from("--settings"),
            config_dir
                .join(harness_connect::CLAUDE_PROFILE_FILE)
                .into_os_string(),
            OsString::from("--model"),
            OsString::from(&model.model),
        ],
        "codex" => vec![
            OsString::from("--profile"),
            OsString::from("alex"),
            OsString::from("--model"),
            OsString::from(&model.model),
        ],
        _ => unreachable!("target validation restricts resume harnesses"),
    };
    let mode = if paste {
        args.push(OsString::from(prompt.replace('\0', "�")));
        ResumeMode::PromptPaste {
            reason: "forced by --paste".into(),
        }
    } else {
        match target {
            "pi" => match sniff_pi_session_format(&config_dir.join("sessions")) {
                Ok(()) => {
                    let draft = build_pi_session_draft(&config_dir, cwd, &model.model, context)?;
                    args.push(OsString::from("--session"));
                    args.push(OsString::from(&draft.id));
                    ResumeMode::NativePi(draft)
                }
                Err(reason) => prompt_paste_mode(&mut args, prompt, reason),
            },
            "claude" => match sniff_claude_session_format(&config_dir.join("projects")) {
                Ok(format) => {
                    let draft = build_claude_session_draft(
                        &config_dir,
                        cwd,
                        &model.model,
                        context,
                        &format,
                    )?;
                    args.push(OsString::from("--resume"));
                    args.push(OsString::from(&draft.id));
                    ResumeMode::NativeClaude(draft)
                }
                Err(reason) => prompt_paste_mode(&mut args, prompt, reason),
            },
            "codex" => match sniff_codex_session_format(&config_dir.join("sessions")) {
                Ok(format) => {
                    let draft = build_codex_session_draft(&config_dir, cwd, context, &format)?;
                    args.push(OsString::from("resume"));
                    args.push(OsString::from(&draft.id));
                    ResumeMode::NativeCodex(draft)
                }
                Err(reason) => prompt_paste_mode(&mut args, prompt, reason),
            },
            _ => unreachable!("target validation restricts resume harnesses"),
        }
    };
    Ok(LaunchPlan {
        harness: target.to_string(),
        binary,
        args,
        cwd: cwd.to_path_buf(),
        config_dir,
        model,
        mode,
    })
}

fn prompt_paste_mode(args: &mut Vec<OsString>, prompt: &str, reason: String) -> ResumeMode {
    args.push(OsString::from(prompt.replace('\0', "�")));
    ResumeMode::PromptPaste { reason }
}

fn select_resume_model(
    target: &str,
    config_dir: &Path,
    source: &ResumeSource,
    requested_model: Option<&str>,
) -> Result<ModelSelection> {
    let models = target_model_ids(target, config_dir);
    if models.is_empty() {
        bail!("{target}'s Alex model catalog is empty; run `alex connect {target}` again");
    }
    if let Some(requested) = requested_model {
        let normalized = alex_model_id(requested);
        if models.contains(&normalized) {
            return Ok(ModelSelection {
                model: normalized,
                reason: None,
            });
        }
        bail!("model {requested} is not available in {target}'s Alex model catalog");
    }

    let default =
        target_default_model(target, config_dir, &models).unwrap_or_else(|| models[0].clone());
    let source_model = source
        .routed_model
        .as_deref()
        .or(source.requested_model.as_deref());
    if let Some(source_model) = source_model {
        let normalized = alex_model_id(source_model);
        if models.contains(&normalized) {
            return Ok(ModelSelection {
                model: normalized,
                reason: None,
            });
        }
        return Ok(ModelSelection {
            model: default.clone(),
            reason: Some(format!(
                "source model {source_model} not available in {target}; using {default}"
            )),
        });
    }
    Ok(ModelSelection {
        model: default.clone(),
        reason: Some(format!(
            "source session did not record a model; using {target}'s current default {default}"
        )),
    })
}

fn target_model_ids(target: &str, config_dir: &Path) -> Vec<String> {
    match target {
        "pi" => harness_connect::read_pi_model_ids(config_dir),
        "claude" => harness_connect::read_claude_model_ids(config_dir),
        "codex" => harness_connect::read_codex_model_ids(config_dir),
        _ => Vec::new(),
    }
}

fn alex_model_id(model: &str) -> String {
    let model = model.trim();
    let bare = ["alex/", "alexandria/", "cove/", "claude-alex/"]
        .iter()
        .find_map(|prefix| model.strip_prefix(prefix))
        .unwrap_or(model);
    format!("alex/{bare}")
}

fn target_default_model(target: &str, config_dir: &Path, models: &[String]) -> Option<String> {
    let configured = match target {
        "pi" => std::fs::read_to_string(config_dir.join("settings.json"))
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .filter(|settings| settings["defaultProvider"].as_str() == Some("alexandria"))
            .and_then(|settings| settings["defaultModel"].as_str().map(String::from)),
        "claude" => std::fs::read_to_string(config_dir.join(harness_connect::CLAUDE_PROFILE_FILE))
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|settings| settings["model"].as_str().map(String::from)),
        "codex" => {
            std::fs::read_to_string(config_dir.join(harness_connect::CODEX_ALEX_PROFILE_FILE))
                .ok()
                .and_then(|raw| DocumentMut::from_str(&raw).ok())
                .and_then(|doc| {
                    doc.get("model")
                        .and_then(|item| item.as_str())
                        .map(String::from)
                })
        }
        _ => None,
    };
    configured
        .map(|model| alex_model_id(&model))
        .filter(|model| models.contains(model))
}

fn sniff_pi_session_format(session_root: &Path) -> std::result::Result<(), String> {
    let recent = most_recent_jsonl(session_root).ok_or_else(|| {
        format!(
            "no existing Pi session under {} was available for the format safety check",
            session_root.display()
        )
    })?;
    let raw = std::fs::read_to_string(&recent).map_err(|error| {
        format!(
            "could not read recent Pi session {}: {error}",
            recent.display()
        )
    })?;
    validate_pi_session_jsonl(&raw)
        .map_err(|reason| format!("recent Pi session format was not recognized: {reason}"))
}

fn most_recent_jsonl(root: &Path) -> Option<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    let mut newest = None;
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file()
                && path.extension().and_then(|extension| extension.to_str()) == Some("jsonl")
            {
                let modified = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok();
                if newest.as_ref().is_none_or(
                    |(current, _): &(Option<std::time::SystemTime>, PathBuf)| modified > *current,
                ) {
                    newest = Some((modified, path));
                }
            }
        }
    }
    newest.map(|(_, path)| path)
}

fn sniff_claude_session_format(
    project_root: &Path,
) -> std::result::Result<ClaudeSessionFormat, String> {
    let recent = most_recent_jsonl(project_root).ok_or_else(|| {
        format!(
            "no existing Claude session under {} was available for the format safety check",
            project_root.display()
        )
    })?;
    let raw = std::fs::read_to_string(&recent).map_err(|error| {
        format!(
            "could not read recent Claude session {}: {error}",
            recent.display()
        )
    })?;
    validate_claude_session_jsonl(&raw)
        .map_err(|reason| format!("recent Claude session format was not recognized: {reason}"))
}

fn validate_claude_session_jsonl(raw: &str) -> std::result::Result<ClaudeSessionFormat, String> {
    let mut version = None;
    let mut conversation_entries = 0usize;
    for (index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value = serde_json::from_str::<Value>(line)
            .map_err(|error| format!("line {} is not JSON: {error}", index + 1))?;
        if !matches!(value["type"].as_str(), Some("user" | "assistant")) {
            continue;
        }
        let line_number = index + 1;
        let kind = value["type"].as_str().unwrap_or_default();
        if value["uuid"].as_str().is_none()
            || !(value["parentUuid"].is_null() || value["parentUuid"].as_str().is_some())
            || value["sessionId"].as_str().is_none()
            || value["timestamp"].as_str().is_none()
            || value["cwd"].as_str().is_none()
            || value["isSidechain"].as_bool().is_none()
        {
            return Err(format!(
                "line {line_number} has an invalid Claude {kind} envelope"
            ));
        }
        let entry_version = value["version"]
            .as_str()
            .filter(|candidate| !candidate.is_empty())
            .ok_or_else(|| format!("line {line_number} has no Claude Code version"))?;
        if version
            .as_deref()
            .is_some_and(|known| known != entry_version)
        {
            return Err(format!(
                "line {line_number} changes Claude Code version within one session"
            ));
        }
        version = Some(entry_version.to_string());
        let message = &value["message"];
        if message["role"].as_str() != Some(kind) {
            return Err(format!(
                "line {line_number} Claude envelope and message roles differ"
            ));
        }
        validate_claude_message_content(message, line_number)?;
        conversation_entries += 1;
    }
    if conversation_entries == 0 {
        return Err("session contains no Claude user or assistant conversation entries".into());
    }
    Ok(ClaudeSessionFormat {
        version: version.unwrap_or_default(),
    })
}

fn validate_claude_message_content(
    message: &Value,
    line: usize,
) -> std::result::Result<(), String> {
    match &message["content"] {
        Value::String(_) => Ok(()),
        Value::Array(blocks) => {
            for block in blocks {
                match block["type"].as_str() {
                    Some("text") if block["text"].as_str().is_some() => {}
                    Some("thinking" | "redacted_thinking") => {}
                    Some("image" | "document") => {}
                    Some("tool_use")
                        if block["id"].as_str().is_some()
                            && block["name"].as_str().is_some()
                            && block["input"].is_object() => {}
                    Some("tool_result")
                        if block["tool_use_id"].as_str().is_some()
                            && (block["content"].is_string() || block["content"].is_array()) => {}
                    kind => {
                        return Err(format!(
                            "line {line} has an unknown Claude {kind:?} content block shape"
                        ))
                    }
                }
            }
            Ok(())
        }
        _ => Err(format!(
            "line {line} Claude message content is not text or an array"
        )),
    }
}

fn sniff_codex_session_format(
    session_root: &Path,
) -> std::result::Result<CodexSessionFormat, String> {
    let recent = most_recent_jsonl(session_root).ok_or_else(|| {
        format!(
            "no existing Codex session under {} was available for the format safety check",
            session_root.display()
        )
    })?;
    let raw = std::fs::read_to_string(&recent).map_err(|error| {
        format!(
            "could not read recent Codex session {}: {error}",
            recent.display()
        )
    })?;
    validate_codex_session_jsonl(&raw)
        .map_err(|reason| format!("recent Codex session format was not recognized: {reason}"))
}

fn validate_codex_session_jsonl(raw: &str) -> std::result::Result<CodexSessionFormat, String> {
    let mut lines = raw.lines().filter(|line| !line.trim().is_empty());
    let first = lines.next().ok_or_else(|| "session is empty".to_string())?;
    let meta = serde_json::from_str::<Value>(first)
        .map_err(|error| format!("line 1 is not JSON: {error}"))?;
    let payload = &meta["payload"];
    if meta["type"].as_str() != Some("session_meta")
        || meta["timestamp"].as_str().is_none()
        || payload["id"].as_str().is_none()
        || payload["session_id"].as_str() != payload["id"].as_str()
        || payload["timestamp"].as_str().is_none()
        || payload["cwd"].as_str().is_none()
        || payload["originator"].as_str().is_none()
        || payload["source"].as_str().is_none()
        || payload["model_provider"].as_str().is_none()
    {
        return Err("expected a Codex session_meta envelope on the first line".into());
    }
    let cli_version = payload["cli_version"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Codex session_meta has no cli_version".to_string())?;
    let history_mode = payload["history_mode"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Codex session_meta has no history_mode".to_string())?;
    if history_mode != "legacy" {
        return Err(format!("unsupported Codex history_mode {history_mode:?}"));
    }

    for (offset, line) in lines.enumerate() {
        let line_number = offset + 2;
        let value = serde_json::from_str::<Value>(line)
            .map_err(|error| format!("line {line_number} is not JSON: {error}"))?;
        if value["type"].as_str() != Some("response_item") {
            continue;
        }
        validate_codex_response_item(&value["payload"], line_number)?;
    }
    Ok(CodexSessionFormat {
        cli_version: cli_version.to_string(),
        history_mode: history_mode.to_string(),
    })
}

fn validate_codex_response_item(payload: &Value, line: usize) -> std::result::Result<(), String> {
    match payload["type"].as_str() {
        Some("message") => {
            if payload["role"].as_str().is_none() || payload["content"].as_array().is_none() {
                return Err(format!("line {line} has an invalid Codex message item"));
            }
            for block in payload["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("input_text" | "output_text") if block["text"].as_str().is_some() => {}
                    kind => {
                        return Err(format!(
                            "line {line} has an unknown Codex {kind:?} message content shape"
                        ))
                    }
                }
            }
        }
        Some("function_call")
            if payload["id"].as_str().is_some()
                && payload["call_id"].as_str().is_some()
                && payload["name"].as_str().is_some()
                && payload["arguments"].as_str().is_some() => {}
        Some("function_call_output")
            if payload["call_id"].as_str().is_some()
                && (payload["output"].is_string() || payload["output"].is_array()) => {}
        Some(
            "reasoning"
            | "agent_message"
            | "custom_tool_call"
            | "custom_tool_call_output"
            | "web_search_call"
            | "computer_call"
            | "computer_call_output"
            | "local_shell_call"
            | "local_shell_call_output"
            | "mcp_tool_call"
            | "mcp_tool_call_output",
        ) => {}
        kind => {
            return Err(format!(
                "line {line} has an unknown Codex response item {kind:?}"
            ))
        }
    }
    Ok(())
}

fn validate_pi_session_jsonl(raw: &str) -> std::result::Result<(), String> {
    let mut values = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value = serde_json::from_str::<Value>(line)
            .map_err(|error| format!("line {} is not JSON: {error}", index + 1))?;
        values.push(value);
    }
    let header = values
        .first()
        .ok_or_else(|| "session is empty".to_string())?;
    if header["type"].as_str() != Some("session")
        || header["version"].as_u64() != Some(PI_SESSION_VERSION)
        || header["id"].as_str().is_none()
        || header["timestamp"].as_str().is_none()
        || header["cwd"].as_str().is_none()
    {
        return Err(format!(
            "expected a Pi v{PI_SESSION_VERSION} session header on the first line"
        ));
    }
    for (offset, entry) in values.iter().enumerate().skip(1) {
        let line = offset + 1;
        let kind = entry["type"]
            .as_str()
            .ok_or_else(|| format!("line {line} has no entry type"))?;
        if entry["id"].as_str().is_none()
            || !(entry["parentId"].is_null() || entry["parentId"].as_str().is_some())
            || entry["timestamp"].as_str().is_none()
        {
            return Err(format!("line {line} has an invalid {kind} entry envelope"));
        }
        match kind {
            "model_change"
                if entry["provider"].as_str().is_some() && entry["modelId"].as_str().is_some() => {}
            "thinking_level_change" if entry["thinkingLevel"].as_str().is_some() => {}
            "message" => validate_pi_message(&entry["message"], line)?,
            "compaction"
                if entry["summary"].as_str().is_some()
                    && entry["firstKeptEntryId"].as_str().is_some()
                    && entry["tokensBefore"].as_u64().is_some() => {}
            "branch_summary"
                if entry["fromId"].as_str().is_some() && entry["summary"].as_str().is_some() => {}
            "custom" if entry["customType"].as_str().is_some() => {}
            "custom_message"
                if entry["customType"].as_str().is_some()
                    && entry["display"].as_bool().is_some()
                    && (entry["content"].is_string() || entry["content"].is_array()) => {}
            "label"
                if entry["targetId"].as_str().is_some()
                    && (entry["label"].is_null() || entry["label"].as_str().is_some()) => {}
            "session_info" if entry["name"].is_null() || entry["name"].as_str().is_some() => {}
            _ => return Err(format!("line {line} has an unknown {kind:?} entry shape")),
        }
    }
    Ok(())
}

fn validate_pi_message(message: &Value, line: usize) -> std::result::Result<(), String> {
    let role = message["role"]
        .as_str()
        .ok_or_else(|| format!("line {line} message has no role"))?;
    if message["timestamp"].as_i64().is_none() && message["timestamp"].as_u64().is_none() {
        return Err(format!("line {line} message has no millisecond timestamp"));
    }
    let content = message["content"]
        .as_array()
        .ok_or_else(|| format!("line {line} {role} content is not an array"))?;
    match role {
        "user" => validate_pi_content(content, line, false),
        "assistant" => {
            if message["api"].as_str().is_none()
                || message["provider"].as_str().is_none()
                || message["model"].as_str().is_none()
                || message["usage"].as_object().is_none()
                || message["stopReason"].as_str().is_none()
            {
                return Err(format!("line {line} assistant metadata is incomplete"));
            }
            validate_pi_content(content, line, true)
        }
        "toolResult" => {
            if message["toolCallId"].as_str().is_none()
                || message["toolName"].as_str().is_none()
                || message["isError"].as_bool().is_none()
            {
                return Err(format!("line {line} toolResult metadata is incomplete"));
            }
            validate_pi_content(content, line, false)
        }
        _ => Err(format!("line {line} has unknown message role {role:?}")),
    }
}

fn validate_pi_content(
    content: &[Value],
    line: usize,
    assistant: bool,
) -> std::result::Result<(), String> {
    for block in content {
        match block["type"].as_str() {
            Some("text") if block["text"].as_str().is_some() => {}
            Some("image")
                if !assistant
                    && block["data"].as_str().is_some()
                    && block["mimeType"].as_str().is_some() => {}
            Some("thinking")
                if assistant
                    && (block["thinking"].as_str().is_some()
                        || block["thinkingSignature"].as_str().is_some()) => {}
            Some("toolCall")
                if assistant
                    && block["id"].as_str().is_some()
                    && block["name"].as_str().is_some()
                    && block["arguments"].as_object().is_some() => {}
            kind => {
                return Err(format!(
                    "line {line} has an unknown {kind:?} content block shape"
                ))
            }
        }
    }
    Ok(())
}

fn build_pi_session_draft(
    config_dir: &Path,
    cwd: &Path,
    model: &str,
    context: &ResumeContext,
) -> Result<PiSessionDraft> {
    let id = Uuid::new_v4().to_string();
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let message_timestamp_ms = Utc::now().timestamp_millis();
    let session_dir = config_dir.join("sessions").join(pi_cwd_slug(cwd));
    let filename_timestamp = timestamp.replace([':', '.'], "-");
    let path = session_dir.join(format!("{filename_timestamp}_{id}.jsonl"));
    let jsonl = render_pi_session(context, cwd, model, &id, &timestamp, message_timestamp_ms)?;
    Ok(PiSessionDraft { id, path, jsonl })
}

fn pi_cwd_slug(cwd: &Path) -> String {
    let path = cwd.to_string_lossy();
    let without_root = path
        .strip_prefix('/')
        .or_else(|| path.strip_prefix('\\'))
        .unwrap_or(&path);
    let safe = without_root
        .chars()
        .map(|ch| {
            if matches!(ch, '/' | '\\' | ':') {
                '-'
            } else {
                ch
            }
        })
        .collect::<String>();
    format!("--{safe}--")
}

struct PiIds {
    session_hex: String,
    entry_index: u32,
    tool_index: u32,
}

impl PiIds {
    fn new(session_id: &str) -> Self {
        Self {
            session_hex: session_id.chars().filter(|ch| *ch != '-').collect(),
            entry_index: 0,
            tool_index: 0,
        }
    }

    fn entry(&mut self) -> String {
        self.entry_index += 1;
        let suffix = self
            .session_hex
            .get(self.session_hex.len().saturating_sub(4)..)
            .unwrap_or("0000");
        format!("{suffix}{:04x}", self.entry_index)
    }

    fn tool_call(&mut self) -> String {
        self.tool_index += 1;
        format!("call_alex_{}_{:04x}", self.session_hex, self.tool_index)
    }
}

fn render_pi_session(
    context: &ResumeContext,
    cwd: &Path,
    model: &str,
    session_id: &str,
    timestamp: &str,
    message_timestamp_ms: i64,
) -> Result<String> {
    let mut ids = PiIds::new(session_id);
    let mut lines = vec![json!({
        "type": "session",
        "version": PI_SESSION_VERSION,
        "id": session_id,
        "timestamp": timestamp,
        "cwd": cwd,
    })];
    let model_entry_id = ids.entry();
    lines.push(json!({
        "type": "model_change",
        "id": model_entry_id,
        "parentId": Value::Null,
        "timestamp": timestamp,
        "provider": "alexandria",
        "modelId": model,
    }));
    let thinking_entry_id = ids.entry();
    lines.push(json!({
        "type": "thinking_level_change",
        "id": thinking_entry_id,
        "parentId": model_entry_id,
        "timestamp": timestamp,
        "thinkingLevel": "off",
    }));

    let mut parent_id = thinking_entry_id;
    let mut tool_calls: HashMap<String, (String, String)> = HashMap::new();
    let mut message_offset = 0i64;
    for entry in &context.entries {
        let messages = pi_messages_from_resume_entry(
            entry,
            model,
            message_timestamp_ms + message_offset,
            &mut ids,
            &mut tool_calls,
        );
        for message in messages {
            message_offset += 1;
            let id = ids.entry();
            lines.push(json!({
                "type": "message",
                "id": id,
                "parentId": parent_id,
                "timestamp": timestamp,
                "message": message,
            }));
            parent_id = id;
        }
    }

    let mut output = String::new();
    for line in lines {
        output.push_str(&serde_json::to_string(&line)?);
        output.push('\n');
    }
    Ok(output)
}

fn pi_messages_from_resume_entry(
    entry: &ResumeEntry,
    model: &str,
    timestamp_ms: i64,
    ids: &mut PiIds,
    tool_calls: &mut HashMap<String, (String, String)>,
) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut buffered = Vec::new();
    for block in &entry.content {
        match block["type"].as_str() {
            Some("text") => {
                if let Some(text) = block["text"].as_str() {
                    buffered.push(json!({"type":"text", "text":text}));
                }
            }
            Some("tool_call") if entry.role == "assistant" => {
                let Some(name) = block["name"].as_str().filter(|name| !name.is_empty()) else {
                    buffered.push(pi_degraded_block("tool call without a name", block));
                    continue;
                };
                let Some(arguments) = block["arguments"].as_object() else {
                    buffered.push(pi_degraded_block(
                        "tool call arguments were not a JSON object",
                        block,
                    ));
                    continue;
                };
                let fresh_id = ids.tool_call();
                if let Some(source_id) = block["id"].as_str().filter(|id| !id.is_empty()) {
                    tool_calls.insert(source_id.to_string(), (fresh_id.clone(), name.to_string()));
                }
                buffered.push(json!({
                    "type":"toolCall",
                    "id":fresh_id,
                    "name":name,
                    "arguments":arguments,
                }));
            }
            Some("tool_result") => {
                let next_timestamp = timestamp_ms + messages.len() as i64;
                flush_pi_message(
                    &mut messages,
                    entry.role,
                    &mut buffered,
                    model,
                    next_timestamp,
                );
                messages.push(pi_tool_result_or_degraded(
                    block,
                    timestamp_ms + messages.len() as i64,
                    tool_calls,
                ));
            }
            Some("content") => buffered.push(pi_degraded_block(
                "source content has no native Pi representation",
                &block["value"],
            )),
            Some(kind) => buffered.push(pi_degraded_block(
                &format!("unsupported source content block {kind}"),
                block,
            )),
            None => buffered.push(pi_degraded_block("source content block had no type", block)),
        }
    }
    let next_timestamp = timestamp_ms + messages.len() as i64;
    flush_pi_message(
        &mut messages,
        entry.role,
        &mut buffered,
        model,
        next_timestamp,
    );
    messages
}

fn flush_pi_message(
    messages: &mut Vec<Value>,
    source_role: &str,
    content: &mut Vec<Value>,
    model: &str,
    timestamp_ms: i64,
) {
    if content.is_empty() {
        return;
    }
    let content = std::mem::take(content);
    if source_role == "assistant" {
        let tool_use = content.iter().any(|block| block["type"] == "toolCall");
        messages.push(json!({
            "role":"assistant",
            "content":content,
            "api":"anthropic-messages",
            "provider":"alexandria",
            "model":model,
            "usage":pi_zero_usage(),
            "stopReason":if tool_use { "toolUse" } else { "stop" },
            "timestamp":timestamp_ms,
        }));
    } else {
        let content = if source_role == "user" {
            content
        } else {
            let mut marked = vec![json!({
                "type":"text",
                "text":format!("[Alex resume: source role {source_role:?} was represented as a Pi user message]")
            })];
            marked.extend(content);
            marked
        };
        messages.push(json!({
            "role":"user",
            "content":content,
            "timestamp":timestamp_ms,
        }));
    }
}

fn pi_tool_result_or_degraded(
    block: &Value,
    timestamp_ms: i64,
    tool_calls: &HashMap<String, (String, String)>,
) -> Value {
    let source_id = block["tool_call_id"].as_str().filter(|id| !id.is_empty());
    if let Some((fresh_id, mapped_name)) = source_id.and_then(|id| tool_calls.get(id)) {
        let name = block["name"].as_str().unwrap_or(mapped_name);
        return json!({
            "role":"toolResult",
            "toolCallId":fresh_id,
            "toolName":name,
            "content":[{"type":"text", "text":pi_result_text(&block["content"])}],
            "isError":block["is_error"].as_bool().unwrap_or(false),
            "timestamp":timestamp_ms,
        });
    }
    json!({
        "role":"user",
        "content":[pi_degraded_block(
            "tool result could not be linked to a representable tool call",
            block,
        )],
        "timestamp":timestamp_ms,
    })
}

fn pi_result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|part| {
                part["text"].as_str().map(String::from).unwrap_or_else(|| {
                    format!("[Alex resume: tool result content block represented as text]\n{part}")
                })
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => format!("[Alex resume: tool result represented as text]\n{other}"),
    }
}

fn pi_degraded_block(reason: &str, original: &Value) -> Value {
    json!({
        "type":"text",
        "text":format!("[Alex resume: {reason}]\n{original}"),
    })
}

fn pi_zero_usage() -> Value {
    json!({
        "input":0,
        "output":0,
        "cacheRead":0,
        "cacheWrite":0,
        "totalTokens":0,
        "cost":{
            "input":0,
            "output":0,
            "cacheRead":0,
            "cacheWrite":0,
            "total":0,
        }
    })
}

fn build_claude_session_draft(
    config_dir: &Path,
    cwd: &Path,
    model: &str,
    context: &ResumeContext,
    format: &ClaudeSessionFormat,
) -> Result<ClaudeSessionDraft> {
    let id = Uuid::new_v4().to_string();
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let path = config_dir
        .join("projects")
        .join(claude_cwd_slug(cwd))
        .join(format!("{id}.jsonl"));
    let jsonl = render_claude_session(context, cwd, model, &id, &format.version, &timestamp)?;
    Ok(ClaudeSessionDraft { id, path, jsonl })
}

fn claude_cwd_slug(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|ch| {
            if matches!(ch, '/' | '\\' | ':') {
                '-'
            } else {
                ch
            }
        })
        .collect()
}

#[derive(Debug)]
struct NativeIds {
    session_hex: String,
    entry_index: u32,
    tool_index: u32,
}

impl NativeIds {
    fn new(session_id: &str) -> Self {
        Self {
            session_hex: session_id.chars().filter(|ch| *ch != '-').collect(),
            entry_index: 0,
            tool_index: 0,
        }
    }

    fn uuid(&mut self) -> String {
        self.entry_index += 1;
        let prefix = self
            .session_hex
            .get(..24)
            .unwrap_or("000000000000400080000000");
        let compact = format!("{prefix}{:08x}", self.entry_index);
        format!(
            "{}-{}-{}-{}-{}",
            &compact[0..8],
            &compact[8..12],
            &compact[12..16],
            &compact[16..20],
            &compact[20..32]
        )
    }

    fn tool(&mut self, prefix: &str) -> String {
        self.tool_index += 1;
        format!("{prefix}_alex_{}_{:04x}", self.session_hex, self.tool_index)
    }
}

#[derive(Debug, Clone)]
struct ClaudeToolLink {
    tool_use_id: String,
    assistant_uuid: String,
}

fn render_claude_session(
    context: &ResumeContext,
    cwd: &Path,
    model: &str,
    session_id: &str,
    version: &str,
    timestamp: &str,
) -> Result<String> {
    let mut ids = NativeIds::new(session_id);
    let mut lines = Vec::new();
    let mut parent_uuid: Option<String> = None;
    let mut tool_calls = HashMap::<String, ClaudeToolLink>::new();

    for entry in &context.entries {
        if entry.role == "assistant" {
            let uuid = ids.uuid();
            let mut content = Vec::new();
            for block in &entry.content {
                match block["type"].as_str() {
                    Some("text") if block["text"].as_str().is_some() => {
                        content.push(json!({"type":"text", "text":block["text"]}));
                    }
                    Some("tool_call") => {
                        let Some(name) = block["name"].as_str().filter(|name| !name.is_empty())
                        else {
                            content.push(native_degraded_block("tool call without a name", block));
                            continue;
                        };
                        let Some(input) = block["arguments"].as_object() else {
                            content.push(native_degraded_block(
                                "tool call arguments were not a JSON object",
                                block,
                            ));
                            continue;
                        };
                        let fresh_id = ids.tool("toolu");
                        if let Some(source_id) = block["id"].as_str().filter(|id| !id.is_empty()) {
                            tool_calls.insert(
                                source_id.to_string(),
                                ClaudeToolLink {
                                    tool_use_id: fresh_id.clone(),
                                    assistant_uuid: uuid.clone(),
                                },
                            );
                        }
                        content.push(json!({
                            "type":"tool_use", "id":fresh_id, "name":name, "input":input
                        }));
                    }
                    Some("tool_result") => content.push(native_degraded_block(
                        "tool result on an assistant entry has no native Claude representation",
                        block,
                    )),
                    Some("content") => content.push(native_degraded_block(
                        "source content has no native Claude representation",
                        &block["value"],
                    )),
                    Some(kind) => content.push(native_degraded_block(
                        &format!("unsupported source content block {kind}"),
                        block,
                    )),
                    None => content.push(native_degraded_block(
                        "source content block had no type",
                        block,
                    )),
                }
            }
            if !content.is_empty() {
                let tool_use = content.iter().any(|block| block["type"] == "tool_use");
                let message = json!({
                    "id":format!("msg_alex_{}_{}", ids.session_hex, ids.entry_index),
                    "type":"message",
                    "role":"assistant",
                    "model":model,
                    "content":content,
                    "stop_reason":if tool_use { "tool_use" } else { "end_turn" },
                    "stop_sequence":Value::Null,
                    "usage":{
                        "input_tokens":0,
                        "output_tokens":0,
                        "cache_creation_input_tokens":0,
                        "cache_read_input_tokens":0
                    }
                });
                let mut line = claude_envelope(
                    "assistant",
                    &uuid,
                    parent_uuid.as_deref(),
                    session_id,
                    version,
                    timestamp,
                    cwd,
                    message,
                );
                line["requestId"] =
                    json!(format!("req_alex_{}_{}", ids.session_hex, ids.entry_index));
                lines.push(line);
                parent_uuid = Some(uuid);
            }
            continue;
        }

        let mut buffered = Vec::new();
        for block in &entry.content {
            match block["type"].as_str() {
                Some("text") if block["text"].as_str().is_some() => {
                    buffered.push(json!({"type":"text", "text":block["text"]}));
                }
                Some("tool_result") => {
                    flush_claude_user_content(
                        &mut lines,
                        &mut buffered,
                        entry.role,
                        &mut ids,
                        &mut parent_uuid,
                        session_id,
                        version,
                        timestamp,
                        cwd,
                    );
                    let source_id = block["tool_call_id"].as_str().filter(|id| !id.is_empty());
                    if let Some(link) = source_id.and_then(|id| tool_calls.get(id)) {
                        let uuid = ids.uuid();
                        let result = pi_result_text(&block["content"]);
                        let message = json!({
                            "role":"user",
                            "content":[{
                                "type":"tool_result",
                                "tool_use_id":link.tool_use_id,
                                "content":result,
                                "is_error":block["is_error"].as_bool().unwrap_or(false)
                            }]
                        });
                        let mut line = claude_envelope(
                            "user",
                            &uuid,
                            parent_uuid.as_deref(),
                            session_id,
                            version,
                            timestamp,
                            cwd,
                            message,
                        );
                        line["sourceToolAssistantUUID"] = json!(link.assistant_uuid);
                        line["toolUseResult"] = if block["content"].is_null() {
                            json!("")
                        } else {
                            block["content"].clone()
                        };
                        line["session_id"] = json!(session_id);
                        lines.push(line);
                        parent_uuid = Some(uuid);
                    } else {
                        buffered.push(native_degraded_block(
                            "tool result could not be linked to a representable tool call",
                            block,
                        ));
                    }
                }
                Some("content") => buffered.push(native_degraded_block(
                    "source content has no native Claude representation",
                    &block["value"],
                )),
                Some("tool_call") => buffered.push(native_degraded_block(
                    "tool call on a non-assistant entry has no native Claude representation",
                    block,
                )),
                Some(kind) => buffered.push(native_degraded_block(
                    &format!("unsupported source content block {kind}"),
                    block,
                )),
                None => buffered.push(native_degraded_block(
                    "source content block had no type",
                    block,
                )),
            }
        }
        flush_claude_user_content(
            &mut lines,
            &mut buffered,
            entry.role,
            &mut ids,
            &mut parent_uuid,
            session_id,
            version,
            timestamp,
            cwd,
        );
    }
    jsonl_string(lines)
}

#[allow(clippy::too_many_arguments)]
fn flush_claude_user_content(
    lines: &mut Vec<Value>,
    content: &mut Vec<Value>,
    source_role: &str,
    ids: &mut NativeIds,
    parent_uuid: &mut Option<String>,
    session_id: &str,
    version: &str,
    timestamp: &str,
    cwd: &Path,
) {
    if content.is_empty() {
        return;
    }
    if source_role != "user" {
        content.insert(
            0,
            json!({
                "type":"text",
                "text":format!(
                    "[Alex resume: source role {source_role:?} was represented as a Claude user message]"
                )
            }),
        );
    }
    let uuid = ids.uuid();
    let line = claude_envelope(
        "user",
        &uuid,
        parent_uuid.as_deref(),
        session_id,
        version,
        timestamp,
        cwd,
        json!({"role":"user", "content":std::mem::take(content)}),
    );
    lines.push(line);
    *parent_uuid = Some(uuid);
}

#[allow(clippy::too_many_arguments)]
fn claude_envelope(
    kind: &str,
    uuid: &str,
    parent_uuid: Option<&str>,
    session_id: &str,
    version: &str,
    timestamp: &str,
    cwd: &Path,
    message: Value,
) -> Value {
    json!({
        "type":kind,
        "uuid":uuid,
        "parentUuid":parent_uuid,
        "sessionId":session_id,
        "version":version,
        "timestamp":timestamp,
        "cwd":cwd,
        "gitBranch":"",
        "isSidechain":false,
        "userType":"external",
        "entrypoint":"cli",
        "message":message,
    })
}

fn build_codex_session_draft(
    config_dir: &Path,
    cwd: &Path,
    context: &ResumeContext,
    format: &CodexSessionFormat,
) -> Result<CodexSessionDraft> {
    let id = Uuid::new_v4().to_string();
    let utc = Utc::now();
    let timestamp = utc.to_rfc3339_opts(SecondsFormat::Millis, true);
    let local = Local::now();
    let path = config_dir
        .join("sessions")
        .join(local.format("%Y/%m/%d").to_string())
        .join(format!(
            "rollout-{}-{id}.jsonl",
            local.format("%Y-%m-%dT%H-%M-%S")
        ));
    let jsonl = render_codex_session(context, cwd, &id, format, &timestamp)?;
    Ok(CodexSessionDraft { id, path, jsonl })
}

fn render_codex_session(
    context: &ResumeContext,
    cwd: &Path,
    session_id: &str,
    format: &CodexSessionFormat,
    timestamp: &str,
) -> Result<String> {
    let mut ids = NativeIds::new(session_id);
    let mut tool_calls = HashMap::<String, String>::new();
    let mut lines = vec![json!({
        "timestamp":timestamp,
        "type":"session_meta",
        "payload":{
            "session_id":session_id,
            "id":session_id,
            "timestamp":timestamp,
            "cwd":cwd,
            "originator":"codex-tui",
            "cli_version":format.cli_version,
            "source":"cli",
            "thread_source":"user",
            "model_provider":"alexandria",
            "history_mode":format.history_mode,
        }
    })];

    for entry in &context.entries {
        let mut text = Vec::new();
        for block in &entry.content {
            match block["type"].as_str() {
                Some("text") if block["text"].as_str().is_some() => {
                    text.push(block["text"].as_str().unwrap_or_default().to_string());
                }
                Some("tool_call") if entry.role == "assistant" => {
                    flush_codex_text(&mut lines, &mut text, entry.role, &mut ids, timestamp);
                    let Some(name) = block["name"].as_str().filter(|name| !name.is_empty()) else {
                        text.push(native_degraded_text("tool call without a name", block));
                        continue;
                    };
                    let call_id = ids.tool("call");
                    if let Some(source_id) = block["id"].as_str().filter(|id| !id.is_empty()) {
                        tool_calls.insert(source_id.to_string(), call_id.clone());
                    }
                    let arguments = serde_json::to_string(&block["arguments"])?;
                    let item_id = format!("fc_alex_{}_{}", ids.session_hex, ids.tool_index);
                    lines.push(codex_response_item(
                        timestamp,
                        json!({
                            "type":"function_call",
                            "id":item_id,
                            "name":name,
                            "arguments":arguments,
                            "call_id":call_id,
                        }),
                    ));
                }
                Some("tool_result") => {
                    flush_codex_text(&mut lines, &mut text, entry.role, &mut ids, timestamp);
                    let source_id = block["tool_call_id"].as_str().filter(|id| !id.is_empty());
                    if let Some(call_id) = source_id.and_then(|id| tool_calls.get(id)) {
                        lines.push(codex_response_item(
                            timestamp,
                            json!({
                                "type":"function_call_output",
                                "call_id":call_id,
                                "output":pi_result_text(&block["content"]),
                            }),
                        ));
                    } else {
                        text.push(native_degraded_text(
                            "tool result could not be linked to a representable tool call",
                            block,
                        ));
                    }
                }
                Some("content") => text.push(native_degraded_text(
                    "source content has no native Codex representation",
                    &block["value"],
                )),
                Some("tool_call") => text.push(native_degraded_text(
                    "tool call on a non-assistant entry has no native Codex representation",
                    block,
                )),
                Some(kind) => text.push(native_degraded_text(
                    &format!("unsupported source content block {kind}"),
                    block,
                )),
                None => text.push(native_degraded_text(
                    "source content block had no type",
                    block,
                )),
            }
        }
        flush_codex_text(&mut lines, &mut text, entry.role, &mut ids, timestamp);
    }
    jsonl_string(lines)
}

fn flush_codex_text(
    lines: &mut Vec<Value>,
    text: &mut Vec<String>,
    source_role: &str,
    ids: &mut NativeIds,
    timestamp: &str,
) {
    if text.is_empty() {
        return;
    }
    let (role, content_type) = if source_role == "assistant" {
        ("assistant", "output_text")
    } else {
        if source_role != "user" {
            text.insert(
                0,
                format!(
                    "[Alex resume: source role {source_role:?} was represented as a Codex user message]"
                ),
            );
        }
        ("user", "input_text")
    };
    let mut payload = json!({
        "type":"message",
        "role":role,
        "content":[{"type":content_type, "text":text.join("\n")}],
    });
    if role == "assistant" {
        payload["id"] = json!(format!(
            "msg_alex_{}_{}",
            ids.session_hex,
            ids.entry_index + 1
        ));
        payload["phase"] = json!("final_answer");
    }
    ids.entry_index += 1;
    lines.push(codex_response_item(timestamp, payload));
    text.clear();
}

fn codex_response_item(timestamp: &str, payload: Value) -> Value {
    json!({"timestamp":timestamp, "type":"response_item", "payload":payload})
}

fn native_degraded_block(reason: &str, original: &Value) -> Value {
    json!({"type":"text", "text":native_degraded_text(reason, original)})
}

fn native_degraded_text(reason: &str, original: &Value) -> String {
    format!("[Alex resume: {reason}]\n{original}")
}

fn jsonl_string(lines: Vec<Value>) -> Result<String> {
    let mut output = String::new();
    for line in lines {
        output.push_str(&serde_json::to_string(&line)?);
        output.push('\n');
    }
    Ok(output)
}

fn print_resume_summary(
    source: &ResumeSource,
    context: &ResumeContext,
    directory: &DirectoryResolution,
    plan: &LaunchPlan,
    dry_run: bool,
) {
    println!(
        "source: {} session {} ({} trace{})",
        source.harness,
        source.session_id,
        source.trace_count,
        if source.trace_count == 1 { "" } else { "s" }
    );
    if let Some(evidence) = &directory.evidence {
        println!(
            "directory: {} ({} from {})",
            directory.path.display(),
            directory.evidence_semantics.unwrap_or("native cwd"),
            evidence.display()
        );
    } else {
        println!(
            "directory: {} (current directory fallback)",
            directory.path.display()
        );
        if let Some(reason) = &directory.fallback_reason {
            println!("  {}", ui::dim(reason));
        }
    }
    println!(
        "context: {} characters, {} entries{}",
        context.prompt.chars().count(),
        context.included_entries,
        if context.truncated {
            format!(", {} older entries omitted", context.omitted_entries)
        } else {
            String::new()
        }
    );
    println!(
        "launch: {} ({}){}",
        plan.harness,
        plan.binary.display(),
        if dry_run { " [dry run]" } else { "" }
    );
    println!("model: {}", plan.model.model);
    if let Some(reason) = &plan.model.reason {
        println!("{reason}");
    }
    match &plan.mode {
        ResumeMode::NativePi(draft) => println!("mode: native pi session {}", draft.id),
        ResumeMode::NativeClaude(draft) => {
            println!("mode: native claude session {}", draft.id)
        }
        ResumeMode::NativeCodex(draft) => {
            println!("mode: native codex session {}", draft.id)
        }
        ResumeMode::PromptPaste { reason } => println!("mode: prompt-paste ({reason})"),
    }
    println!("config: {}", plan.config_dir.display());
    for warning in &source.warnings {
        println!("warning: {warning}");
    }
}

async fn launch_and_record_fork(
    store: &Store,
    source: &ResumeSource,
    directory: &DirectoryResolution,
    plan: LaunchPlan,
    token: &str,
) -> Result<()> {
    let native_target_session = match &plan.mode {
        ResumeMode::NativePi(draft) => {
            materialize_native_session(&draft.path, &draft.jsonl, "Pi")?;
            Some(draft.id.clone())
        }
        ResumeMode::NativeClaude(draft) => {
            materialize_native_session(&draft.path, &draft.jsonl, "Claude")?;
            Some(draft.id.clone())
        }
        ResumeMode::NativeCodex(draft) => {
            materialize_native_session(&draft.path, &draft.jsonl, "Codex")?;
            Some(draft.id.clone())
        }
        ResumeMode::PromptPaste { .. } => None,
    };
    let started_ms = now_ms();
    let mut child = Command::new(&plan.binary)
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("could not launch {}", plan.binary.display()))?;

    let mut recorded = false;
    if let Some(target_session_id) = native_target_session.as_deref() {
        record_fork(store, source, directory, &plan.harness, target_session_id)?;
        eprintln!(
            "Alex recorded fork {} → {}",
            source.session_id, target_session_id
        );
        recorded = true;
    }
    let status = loop {
        if !recorded {
            if let Some(target_session_id) =
                find_fork_target(store, &plan.harness, token, started_ms)?
            {
                record_fork(store, source, directory, &plan.harness, &target_session_id)?;
                eprintln!(
                    "Alex recorded fork {} → {}",
                    source.session_id, target_session_id
                );
                recorded = true;
            }
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };

    // The daemon can commit the final request just after the harness exits.
    if !recorded {
        for _ in 0..8 {
            if let Some(target_session_id) =
                find_fork_target(store, &plan.harness, token, started_ms)?
            {
                record_fork(store, source, directory, &plan.harness, &target_session_id)?;
                recorded = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    if !recorded {
        eprintln!(
            "warning: the harness exited before Alex observed its new session; fork lineage was not recorded"
        );
    }
    exit_status(status)
}

fn materialize_native_session(path: &Path, jsonl: &str, harness: &str) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("native {harness} session path has no parent directory"))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating {harness} session directory {}", parent.display()))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).with_context(|| {
        format!(
            "creating fresh native {harness} session {} without overwriting existing files",
            path.display()
        )
    })?;
    file.write_all(jsonl.as_bytes())
        .with_context(|| format!("writing native {harness} session {}", path.display()))?;
    file.flush()
        .with_context(|| format!("flushing native {harness} session {}", path.display()))?;
    Ok(())
}

fn record_fork(
    store: &Store,
    source: &ResumeSource,
    directory: &DirectoryResolution,
    target_harness: &str,
    target_session_id: &str,
) -> Result<()> {
    store.record_session_fork(&SessionForkRecord {
        source_session_id: source.session_id.clone(),
        source_harness: source.harness.clone(),
        target_session_id: target_session_id.to_string(),
        target_harness: target_harness.to_string(),
        created_ms: now_ms(),
        recovered_cwd: directory
            .evidence
            .as_ref()
            .map(|_| directory.path.to_string_lossy().into_owned()),
    })?;
    Ok(())
}

fn exit_status(status: ExitStatus) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        bail!("harness exited with {status}")
    }
}

fn find_fork_target(
    store: &Store,
    target_harness: &str,
    token: &str,
    since_ms: i64,
) -> Result<Option<String>> {
    let rows = store.search_traces(&TraceFilter {
        since_ms: Some(since_ms.saturating_sub(1_000)),
        harness: Some(target_harness.to_string()),
        limit: FORK_DISCOVERY_LIMIT,
        ..Default::default()
    })?;
    for row in rows {
        if row["harness"].as_str().map(canonical_harness).as_deref() != Some(target_harness) {
            continue;
        }
        let Some(path) = row["req_body_path"].as_str() else {
            continue;
        };
        let Ok(body) = read_gzip(path) else {
            continue;
        };
        if body
            .windows(token.len())
            .any(|window| window == token.as_bytes())
        {
            return Ok(row["session_id"].as_str().map(String::from));
        }
    }
    Ok(None)
}

#[derive(Debug)]
struct DirectoryCandidate {
    path: PathBuf,
    evidence: PathBuf,
    semantics: &'static str,
}

fn recover_directory(config: &Config, source: &ResumeSource) -> Result<DirectoryResolution> {
    let current = std::env::current_dir().context("reading current directory")?;
    let Some(spec) = harness_connect::spec_by_name(&source.harness) else {
        return Ok(DirectoryResolution {
            path: current,
            evidence: None,
            evidence_semantics: None,
            fallback_reason: Some(format!(
                "{} does not expose a supported native session directory",
                source.harness
            )),
        });
    };
    let config_dir = harness_connect::resolve_config_dir(config, spec, None);
    let candidates = match source.harness.as_str() {
        "pi" => find_native_session_candidates(
            &config_dir.join("sessions"),
            &source.session_id,
            NativeSessionKind::Pi,
            "session cwd",
        )?,
        "claude" => find_native_session_candidates(
            &config_dir.join("projects"),
            &source.session_id,
            NativeSessionKind::Claude,
            "latest native cwd",
        )?,
        "codex" => {
            let mut candidates = find_codex_state_candidates(&config_dir, &source.session_id);
            candidates.extend(find_native_session_candidates(
                &config_dir.join("sessions"),
                &source.session_id,
                NativeSessionKind::Codex,
                "original native cwd",
            )?);
            candidates.extend(find_native_session_candidates(
                &config_dir.join("archived_sessions"),
                &source.session_id,
                NativeSessionKind::Codex,
                "original archived cwd",
            )?);
            candidates
        }
        _ => Vec::new(),
    };

    if let Some(candidate) = candidates.iter().find(|candidate| candidate.path.is_dir()) {
        return Ok(DirectoryResolution {
            path: candidate
                .path
                .canonicalize()
                .unwrap_or_else(|_| candidate.path.clone()),
            evidence: Some(candidate.evidence.clone()),
            evidence_semantics: Some(candidate.semantics),
            fallback_reason: None,
        });
    }

    let fallback_reason = candidates.first().map_or_else(
        || {
            format!(
                "no exact {} native session record was found for {} under {}",
                source.harness,
                source.session_id,
                config_dir.display()
            )
        },
        |candidate| {
            format!(
                "native session metadata at {} recorded {}, but that directory no longer exists",
                candidate.evidence.display(),
                candidate.path.display()
            )
        },
    );
    Ok(DirectoryResolution {
        path: current,
        evidence: None,
        evidence_semantics: None,
        fallback_reason: Some(fallback_reason),
    })
}

#[derive(Clone, Copy)]
enum NativeSessionKind {
    Pi,
    Claude,
    Codex,
}

fn find_native_session_candidates(
    root: &Path,
    session_id: &str,
    kind: NativeSessionKind,
    semantics: &'static str,
) -> Result<Vec<DirectoryCandidate>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut pending = vec![root.to_path_buf()];
    let mut matched_files = Vec::new();
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                pending.push(path);
                continue;
            }
            if !file_type.is_file() || !native_filename_matches(&path, session_id, kind) {
                continue;
            }
            matched_files.push(path);
        }
    }
    matched_files.sort();
    let mut candidates = Vec::new();
    for path in matched_files {
        let mut directories = native_file_cwds(&path, session_id, kind)?;
        if matches!(kind, NativeSessionKind::Claude) {
            directories.reverse();
        }
        candidates.extend(directories.into_iter().map(|directory| DirectoryCandidate {
            path: directory,
            evidence: path.clone(),
            semantics,
        }));
    }
    Ok(candidates)
}

fn find_codex_state_candidates(config_dir: &Path, session_id: &str) -> Vec<DirectoryCandidate> {
    let Ok(entries) = std::fs::read_dir(config_dir) else {
        return Vec::new();
    };
    let mut databases = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let version = name
                .strip_prefix("state_")?
                .strip_suffix(".sqlite")?
                .parse::<u64>()
                .ok()?;
            entry.file_type().ok()?.is_file().then_some((version, path))
        })
        .collect::<Vec<_>>();
    databases.sort_by_key(|(version, _)| std::cmp::Reverse(*version));

    for (_, path) in databases {
        let Ok(connection) = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) else {
            continue;
        };
        let cwd = connection
            .query_row(
                "SELECT cwd FROM threads WHERE id = ?1 LIMIT 1",
                [session_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional();
        let Ok(Some(Some(cwd))) = cwd else {
            continue;
        };
        let cwd = PathBuf::from(cwd);
        if cwd.is_absolute() {
            return vec![DirectoryCandidate {
                path: cwd,
                evidence: path,
                semantics: "latest native cwd",
            }];
        }
    }
    Vec::new()
}

fn native_filename_matches(path: &Path, session_id: &str, kind: NativeSessionKind) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    match kind {
        NativeSessionKind::Pi => name.ends_with(&format!("_{session_id}.jsonl")),
        NativeSessionKind::Claude => {
            name == format!("{session_id}.jsonl") || name == format!("agent-{session_id}.jsonl")
        }
        NativeSessionKind::Codex => name.ends_with(&format!("-{session_id}.jsonl")),
    }
}

fn native_file_cwds(
    path: &Path,
    session_id: &str,
    kind: NativeSessionKind,
) -> Result<Vec<PathBuf>> {
    let file = File::open(path)?;
    let mut directories = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let cwd = match kind {
            NativeSessionKind::Pi
                if value["type"].as_str() == Some("session")
                    && value["id"].as_str() == Some(session_id) =>
            {
                value["cwd"].as_str()
            }
            NativeSessionKind::Claude
                if value["sessionId"].as_str() == Some(session_id)
                    || value["agentId"].as_str() == Some(session_id) =>
            {
                value["cwd"].as_str()
            }
            NativeSessionKind::Codex
                if value["type"].as_str() == Some("session_meta")
                    && value["payload"]["id"].as_str() == Some(session_id) =>
            {
                value["payload"]["cwd"].as_str()
            }
            _ => None,
        };
        if let Some(cwd) = cwd.filter(|cwd| Path::new(cwd).is_absolute()) {
            let cwd = PathBuf::from(cwd);
            if directories.last() != Some(&cwd) {
                directories.push(cwd);
            }
            if !matches!(kind, NativeSessionKind::Claude) {
                break;
            }
        }
    }
    Ok(directories)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alex_core::TraceRecord;

    const CLAUDE_REAL_SHAPE_FIXTURE: &str = concat!(
        r#"{"type":"system","subtype":"away_summary","content":"fixture summary","isMeta":false,"uuid":"00000000-0000-4000-8000-000000000000","parentUuid":null,"sessionId":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","version":"2.1.215","timestamp":"2026-07-20T01:02:03.000Z","cwd":"/fixture/project","gitBranch":"main","isSidechain":false,"userType":"external","entrypoint":"cli"}"#,
        "\n",
        r#"{"type":"user","uuid":"10000000-0000-4000-8000-000000000001","parentUuid":null,"sessionId":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","version":"2.1.215","timestamp":"2026-07-20T01:02:03.001Z","cwd":"/fixture/project","gitBranch":"main","isSidechain":false,"userType":"external","entrypoint":"cli","message":{"role":"user","content":"inspect it"}}"#,
        "\n",
        r#"{"type":"assistant","uuid":"20000000-0000-4000-8000-000000000002","parentUuid":"10000000-0000-4000-8000-000000000001","sessionId":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","version":"2.1.215","timestamp":"2026-07-20T01:02:03.002Z","cwd":"/fixture/project","gitBranch":"main","isSidechain":false,"userType":"external","entrypoint":"cli","requestId":"req_fixture","message":{"id":"msg_fixture","type":"message","role":"assistant","model":"claude-fable-5","content":[{"type":"tool_use","id":"toolu_fixture","name":"Read","input":{"file_path":"src/main.rs"}}],"stop_reason":"tool_use","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}}"#,
        "\n",
        r#"{"type":"user","uuid":"30000000-0000-4000-8000-000000000003","parentUuid":"20000000-0000-4000-8000-000000000002","sessionId":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","session_id":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","version":"2.1.215","timestamp":"2026-07-20T01:02:03.003Z","cwd":"/fixture/project","gitBranch":"main","isSidechain":false,"userType":"external","entrypoint":"cli","sourceToolAssistantUUID":"20000000-0000-4000-8000-000000000002","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_fixture","content":"fn main() {}"}]},"toolUseResult":{"content":"fn main() {}","filePath":"src/main.rs","type":"text"}}"#,
        "\n"
    );

    const CODEX_REAL_SHAPE_FIXTURE: &str = concat!(
        r#"{"timestamp":"2026-07-20T01:02:03.000Z","type":"session_meta","payload":{"session_id":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","id":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","timestamp":"2026-07-20T01:02:03.000Z","cwd":"/fixture/project","originator":"codex-tui","cli_version":"0.144.6","source":"cli","thread_source":"user","model_provider":"alexandria","history_mode":"legacy"}}"#,
        "\n",
        r#"{"timestamp":"2026-07-20T01:02:03.001Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"inspect it"}]}}"#,
        "\n",
        r#"{"timestamp":"2026-07-20T01:02:03.002Z","type":"response_item","payload":{"type":"message","role":"assistant","phase":"commentary","id":"msg_fixture","content":[{"type":"output_text","text":"I'll inspect."}]}}"#,
        "\n",
        r#"{"timestamp":"2026-07-20T01:02:03.003Z","type":"response_item","payload":{"type":"function_call","id":"fc_fixture","name":"read","arguments":"{\"path\":\"src/main.rs\"}","call_id":"call_fixture"}}"#,
        "\n",
        r#"{"timestamp":"2026-07-20T01:02:03.004Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_fixture","output":[{"type":"input_text","text":"fn main() {}"}]}}"#,
        "\n"
    );

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "alex-resume-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn native_writer_context() -> ResumeContext {
        ResumeContext {
            prompt: "fixture".into(),
            entries: vec![
                ResumeEntry {
                    role: "user",
                    content: vec![json!({"type":"text", "text":"inspect it"})],
                },
                ResumeEntry {
                    role: "assistant",
                    content: vec![
                        json!({"type":"text", "text":"I'll inspect."}),
                        json!({
                            "type":"tool_call",
                            "id":"source-call-1",
                            "name":"read",
                            "arguments":{"path":"src/main.rs"}
                        }),
                    ],
                },
                ResumeEntry {
                    role: "tool",
                    content: vec![json!({
                        "type":"tool_result",
                        "tool_call_id":"source-call-1",
                        "name":"read",
                        "content":"fn main() {}",
                        "is_error":false
                    })],
                },
                ResumeEntry {
                    role: "assistant",
                    content: vec![json!({"type":"text", "text":"It is a small program."})],
                },
            ],
            truncated: false,
            omitted_entries: 0,
            included_entries: 4,
            original_chars: 7,
            prompt_chars: 7,
        }
    }

    #[test]
    fn cli_parses_optional_harness_source_and_dry_run() {
        use clap::Parser;

        let cli = crate::Cli::try_parse_from([
            "alex",
            "resume",
            "session-1",
            "pi",
            "--source-harness",
            "codex",
            "--model",
            "gpt-5.6-sol",
            "--paste",
            "--dry-run",
        ])
        .unwrap();
        match cli.command.unwrap() {
            crate::Command::Resume {
                session,
                harness,
                source_harness,
                model,
                paste,
                dry_run,
            } => {
                assert_eq!(session, "session-1");
                assert_eq!(harness.as_deref(), Some("pi"));
                assert_eq!(source_harness.as_deref(), Some("codex"));
                assert_eq!(model.as_deref(), Some("gpt-5.6-sol"));
                assert!(paste);
                assert!(dry_run);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn pi_writer_emits_v3_native_history_with_fresh_linked_tool_ids() {
        let context = native_writer_context();
        let session_id = "11111111-2222-3333-4444-555555556666";
        let timestamp = "2026-07-20T01:02:03.004Z";
        let rendered = render_pi_session(
            &context,
            Path::new("/fixture/project"),
            "alex/gpt-5.6-sol",
            session_id,
            timestamp,
            1_750_000_000_000,
        )
        .unwrap();
        let lines = rendered
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(lines.len(), 7);
        assert_eq!(
            lines[0],
            json!({
                "type":"session", "version":3, "id":session_id,
                "timestamp":timestamp, "cwd":"/fixture/project"
            })
        );
        assert_eq!(
            lines[1],
            json!({
                "type":"model_change", "id":"66660001", "parentId":null,
                "timestamp":timestamp, "provider":"alexandria", "modelId":"alex/gpt-5.6-sol"
            })
        );
        assert_eq!(lines[2]["type"], "thinking_level_change");
        assert_eq!(lines[3]["message"]["role"], "user");
        assert_eq!(lines[3]["parentId"], lines[2]["id"]);
        assert_eq!(lines[4]["message"]["role"], "assistant");
        assert_eq!(
            lines[4]["message"]["content"][1],
            json!({
                "type":"toolCall",
                "id":format!("call_alex_{}_0001", session_id.replace('-', "")),
                "name":"read",
                "arguments":{"path":"src/main.rs"}
            })
        );
        assert_eq!(lines[5]["message"]["role"], "toolResult");
        assert_eq!(
            lines[5]["message"]["toolCallId"],
            lines[4]["message"]["content"][1]["id"]
        );
        assert_eq!(
            lines[6]["message"]["content"][0]["text"],
            "It is a small program."
        );
        assert!(!rendered.contains("source-call-1"));
        validate_pi_session_jsonl(&rendered).unwrap();
    }

    #[test]
    fn claude_real_shape_fixture_and_golden_writer_preserve_linked_tool_turn() {
        assert_eq!(
            validate_claude_session_jsonl(CLAUDE_REAL_SHAPE_FIXTURE).unwrap(),
            ClaudeSessionFormat {
                version: "2.1.215".into()
            }
        );

        let session_id = "11111111-2222-4333-8444-555555556666";
        let timestamp = "2026-07-20T01:02:03.004Z";
        let rendered = render_claude_session(
            &native_writer_context(),
            Path::new("/fixture/project"),
            "alex/claude-fable-5",
            session_id,
            "2.1.215",
            timestamp,
        )
        .unwrap();
        let lines = rendered
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0]["type"], "user");
        assert!(lines[0]["parentUuid"].is_null());
        assert_eq!(lines[0]["sessionId"], session_id);
        assert_eq!(lines[0]["version"], "2.1.215");
        assert_eq!(lines[1]["type"], "assistant");
        assert_eq!(lines[1]["parentUuid"], lines[0]["uuid"]);
        assert_eq!(
            lines[1]["message"]["content"][1],
            json!({
                "type":"tool_use",
                "id":format!("toolu_alex_{}_0001", session_id.replace('-', "")),
                "name":"read",
                "input":{"path":"src/main.rs"}
            })
        );
        assert_eq!(lines[2]["type"], "user");
        assert_eq!(lines[2]["parentUuid"], lines[1]["uuid"]);
        assert_eq!(lines[2]["sourceToolAssistantUUID"], lines[1]["uuid"]);
        assert_eq!(
            lines[2]["message"]["content"][0]["tool_use_id"],
            lines[1]["message"]["content"][1]["id"]
        );
        assert_eq!(lines[2]["toolUseResult"], "fn main() {}");
        assert_eq!(lines[3]["parentUuid"], lines[2]["uuid"]);
        assert!(!rendered.contains("source-call-1"));
        validate_claude_session_jsonl(&rendered).unwrap();
    }

    #[test]
    fn codex_real_shape_fixture_and_golden_writer_preserve_linked_tool_turn() {
        assert_eq!(
            validate_codex_session_jsonl(CODEX_REAL_SHAPE_FIXTURE).unwrap(),
            CodexSessionFormat {
                cli_version: "0.144.6".into(),
                history_mode: "legacy".into(),
            }
        );

        let session_id = "11111111-2222-4333-8444-555555556666";
        let timestamp = "2026-07-20T01:02:03.004Z";
        let rendered = render_codex_session(
            &native_writer_context(),
            Path::new("/fixture/project"),
            session_id,
            &CodexSessionFormat {
                cli_version: "0.144.6".into(),
                history_mode: "legacy".into(),
            },
            timestamp,
        )
        .unwrap();
        let lines = rendered
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0]["type"], "session_meta");
        assert_eq!(lines[0]["payload"]["id"], session_id);
        assert_eq!(lines[0]["payload"]["cli_version"], "0.144.6");
        assert_eq!(lines[1]["payload"]["role"], "user");
        assert_eq!(lines[2]["payload"]["role"], "assistant");
        assert_eq!(lines[3]["payload"]["type"], "function_call");
        assert_eq!(
            lines[3]["payload"]["arguments"],
            r#"{"path":"src/main.rs"}"#
        );
        assert_eq!(lines[4]["payload"]["type"], "function_call_output");
        assert_eq!(
            lines[4]["payload"]["call_id"],
            lines[3]["payload"]["call_id"]
        );
        assert_eq!(lines[5]["payload"]["role"], "assistant");
        assert!(!rendered.contains("source-call-1"));
        validate_codex_session_jsonl(&rendered).unwrap();
    }

    #[test]
    fn source_model_mismatch_falls_back_to_target_default_with_reason() {
        let config_dir = tmpdir("model-fallback");
        std::fs::write(
            config_dir.join("models.json"),
            json!({
                "providers": {"alexandria": {"models": [
                    {"id":"alex/claude-sonnet-5"},
                    {"id":"alex/gpt-5.6-sol"}
                ]}}
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            config_dir.join("settings.json"),
            json!({"defaultProvider":"alexandria", "defaultModel":"gpt-5.6-sol"}).to_string(),
        )
        .unwrap();
        let source = ResumeSource {
            session_id: "source".into(),
            harness: "claude".into(),
            captures: Vec::new(),
            requested_model: Some("claude-fable-5".into()),
            routed_model: None,
            trace_count: 1,
            warnings: Vec::new(),
        };

        let selected = select_resume_model("pi", &config_dir, &source, None).unwrap();
        assert_eq!(selected.model, "alex/gpt-5.6-sol");
        assert_eq!(
            selected.reason.as_deref(),
            Some("source model claude-fable-5 not available in pi; using alex/gpt-5.6-sol")
        );
    }

    #[test]
    fn pi_version_sniff_rejects_unknown_recent_shape_without_reading_home() {
        let root = tmpdir("pi-version-sniff");
        let sessions = root.join("sessions").join("--fixture--");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("recent.jsonl"),
            concat!(
                "{\"type\":\"session\",\"version\":4,\"id\":\"future\",",
                "\"timestamp\":\"2026-07-20T00:00:00.000Z\",\"cwd\":\"/fixture\"}\n"
            ),
        )
        .unwrap();

        let reason = sniff_pi_session_format(&root.join("sessions")).unwrap_err();
        assert!(reason.contains("format was not recognized"));
        assert!(reason.contains("expected a Pi v3 session header"));
    }

    #[test]
    fn claude_version_sniff_rejects_unknown_recent_shape_without_reading_home() {
        let root = tmpdir("claude-version-sniff");
        let projects = root.join("projects").join("-fixture-project");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::write(
            projects.join("recent.jsonl"),
            concat!(
                "{\"type\":\"user\",\"version\":\"99.0.0\",",
                "\"sessionId\":\"future\",\"timestamp\":\"2026-07-20T00:00:00.000Z\",",
                "\"cwd\":\"/fixture\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n"
            ),
        )
        .unwrap();

        let reason = sniff_claude_session_format(&root.join("projects")).unwrap_err();
        assert!(reason.contains("format was not recognized"));
        assert!(reason.contains("invalid Claude user envelope"));
    }

    #[test]
    fn codex_version_sniff_rejects_unknown_recent_shape_without_reading_home() {
        let root = tmpdir("codex-version-sniff");
        let sessions = root.join("sessions").join("2026/07/20");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("recent.jsonl"),
            concat!(
                "{\"timestamp\":\"2026-07-20T00:00:00.000Z\",\"type\":\"session_meta\",",
                "\"payload\":{\"session_id\":\"future\",\"id\":\"future\",",
                "\"timestamp\":\"2026-07-20T00:00:00.000Z\",\"cwd\":\"/fixture\",",
                "\"originator\":\"codex-tui\",\"cli_version\":\"99.0.0\",",
                "\"source\":\"cli\",\"model_provider\":\"alexandria\",",
                "\"history_mode\":\"signed-v99\"}}\n"
            ),
        )
        .unwrap();

        let reason = sniff_codex_session_format(&root.join("sessions")).unwrap_err();
        assert!(reason.contains("format was not recognized"));
        assert!(reason.contains("unsupported Codex history_mode"));
    }

    #[test]
    fn pi_cwd_slug_matches_native_pi_encoding() {
        assert_eq!(pi_cwd_slug(Path::new("/private/tmp")), "--private-tmp--");
        assert_eq!(
            pi_cwd_slug(Path::new("/Users/example/project")),
            "--Users-example-project--"
        );
    }

    #[test]
    fn claude_cwd_slug_matches_native_claude_encoding() {
        assert_eq!(claude_cwd_slug(Path::new("/private/tmp")), "-private-tmp");
        assert_eq!(
            claude_cwd_slug(Path::new("/Users/example/project")),
            "-Users-example-project"
        );
    }

    #[test]
    fn native_materialization_never_overwrites_an_existing_session() {
        let root = tmpdir("native-create-new");
        let path = root.join("sessions/fresh.jsonl");
        materialize_native_session(&path, "first\n", "fixture").unwrap();
        let error = materialize_native_session(&path, "second\n", "fixture").unwrap_err();
        assert!(error
            .to_string()
            .contains("without overwriting existing files"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "first\n");
    }

    #[test]
    fn native_metadata_requires_exact_session_identity() {
        let root = tmpdir("native-metadata");
        let cwd = root.join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let pi = root.join("2026-01-01_session-1.jsonl");
        std::fs::write(
            &pi,
            format!(
                "{}\n",
                serde_json::json!({"type":"session","id":"session-1","cwd":cwd})
            ),
        )
        .unwrap();
        assert_eq!(
            native_file_cwds(&pi, "session-1", NativeSessionKind::Pi).unwrap(),
            vec![cwd]
        );
        assert!(
            native_file_cwds(&pi, "mentioned-elsewhere", NativeSessionKind::Pi)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn claude_prefers_the_latest_exact_native_cwd() {
        let root = tmpdir("claude-latest-cwd");
        let first = root.join("first");
        let latest = root.join("latest");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&latest).unwrap();
        let projects = root.join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        let session = projects.join("session-1.jsonl");
        std::fs::write(
            &session,
            format!(
                "{}\n{}\n{}\n",
                serde_json::json!({"sessionId":"session-1","cwd":first}),
                serde_json::json!({"sessionId":"other-session","cwd":"/wrong"}),
                serde_json::json!({"sessionId":"session-1","cwd":latest}),
            ),
        )
        .unwrap();

        let candidates = find_native_session_candidates(
            &projects,
            "session-1",
            NativeSessionKind::Claude,
            "latest native cwd",
        )
        .unwrap();
        assert_eq!(candidates[0].path, latest);
        assert_eq!(candidates[1].path, first);
        assert_eq!(candidates[0].evidence, session);
    }

    #[test]
    fn codex_prefers_the_highest_state_database() {
        let root = tmpdir("codex-state");
        let stale = root.join("stale");
        let latest = root.join("latest");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::create_dir_all(&latest).unwrap();
        for (version, cwd) in [(3, &stale), (12, &latest)] {
            let database = root.join(format!("state_{version}.sqlite"));
            let connection = Connection::open(database).unwrap();
            connection
                .execute_batch("CREATE TABLE threads (id TEXT PRIMARY KEY, cwd TEXT)")
                .unwrap();
            connection
                .execute(
                    "INSERT INTO threads (id, cwd) VALUES (?1, ?2)",
                    rusqlite::params!["session-1", cwd.to_string_lossy()],
                )
                .unwrap();
        }

        let candidates = find_codex_state_candidates(&root, "session-1");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].path, latest);
        assert!(candidates[0].evidence.ends_with("state_12.sqlite"));
    }

    #[test]
    fn source_loader_rejects_session_ids_shared_by_harnesses() {
        let dir = tmpdir("ambiguous");
        let store = Store::open(dir).unwrap();
        for (id, harness) in [("trace-pi", "pi"), ("trace-codex", "codex")] {
            let body = store
                .write_body(id, "request.json", br#"{"messages":[]}"#)
                .unwrap();
            let mut trace = TraceRecord {
                id: id.into(),
                ts_request_ms: 1,
                ..Default::default()
            };
            trace.session_id = Some("shared".into());
            trace.harness = Some(harness.into());
            trace.client_format = Some("anthropic".into());
            trace.upstream_format = Some("anthropic".into());
            trace.req_body_path = Some(body);
            store.insert_trace(&trace).unwrap();
        }
        let error = load_resume_source(&store, "shared", None).unwrap_err();
        assert!(error.to_string().contains("--source-harness"));
    }

    #[test]
    fn source_loader_stitches_stateless_captures_in_trace_order() {
        let dir = tmpdir("stateless-captures");
        let store = Store::open(dir).unwrap();
        for (index, (user, assistant)) in [
            ("first question", "first answer"),
            ("second question", "second answer"),
        ]
        .into_iter()
        .enumerate()
        {
            let id = format!("trace-{index}");
            let request = serde_json::json!({
                "messages": [{"role":"user", "content":user}]
            });
            let response = serde_json::json!({
                "role":"assistant",
                "content":[{"type":"text", "text":assistant}]
            });
            let request_path = store
                .write_body(&id, "request.json", request.to_string().as_bytes())
                .unwrap();
            let response_path = store
                .write_body(&id, "response.json", response.to_string().as_bytes())
                .unwrap();
            let trace = TraceRecord {
                id,
                ts_request_ms: index as i64 + 1,
                session_id: Some("stateless-session".into()),
                harness: Some("claude".into()),
                client_format: Some("anthropic".into()),
                upstream_format: Some("anthropic".into()),
                req_body_path: Some(request_path),
                resp_body_path: Some(response_path),
                ..Default::default()
            };
            store.insert_trace(&trace).unwrap();
        }

        let source = load_resume_source(&store, "stateless-session", None).unwrap();
        assert_eq!(source.captures.len(), 2);
        let (_, prompt) = build_fork_context(&source, "fork-token");
        let positions = [
            "first question",
            "first answer",
            "second question",
            "second answer",
        ]
        .map(|needle| prompt.find(needle).unwrap());
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn fork_prompt_stays_below_the_single_argument_transport_limit() {
        let source = ResumeSource {
            session_id: "large-session".into(),
            harness: "codex".into(),
            captures: vec![CapturedExchange {
                client_format: ClientFormat::OpenaiChat,
                request: serde_json::json!({
                    "messages": [
                        {"role":"user", "content":"x".repeat(RESUME_PROMPT_MAX_BYTES * 2)},
                        {"role":"user", "content":"latest small request"}
                    ]
                }),
                response_format: ClientFormat::OpenaiChat,
                response: serde_json::json!({
                    "choices":[{"message":{"role":"assistant", "content":"latest answer"}}]
                })
                .to_string(),
            }],
            requested_model: None,
            routed_model: None,
            trace_count: 1,
            warnings: Vec::new(),
        };

        let (context, prompt) = build_fork_context(&source, "fork-token");
        assert!(context.truncated);
        assert!(prompt.len() <= RESUME_PROMPT_MAX_BYTES);
        assert!(!prompt.contains(&"x".repeat(1_000)));
        assert!(prompt.contains("latest small request"));
        assert!(prompt.contains("latest answer"));
    }

    #[test]
    fn fork_target_matches_unique_marker_in_request_body() {
        let dir = tmpdir("fork-target");
        let store = Store::open(dir).unwrap();
        let body = store
            .write_body(
                "trace-1",
                "request.json",
                br#"{"input":"context token-123"}"#,
            )
            .unwrap();
        let mut trace = TraceRecord {
            id: "trace-1".into(),
            ts_request_ms: 10,
            ..Default::default()
        };
        trace.session_id = Some("new-session".into());
        trace.harness = Some("pi".into());
        trace.req_body_path = Some(body);
        store.insert_trace(&trace).unwrap();
        assert_eq!(
            find_fork_target(&store, "pi", "token-123", 1).unwrap(),
            Some("new-session".into())
        );
        assert_eq!(find_fork_target(&store, "pi", "missing", 1).unwrap(), None);
    }
}
