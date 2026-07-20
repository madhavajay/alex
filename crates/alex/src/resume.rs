use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

use alex_core::{build_resume_context_from_captures, ClientFormat, ResumeCapture, ResumeContext};
use alex_store::{SessionForkRecord, Store, TraceFilter};
use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use uuid::Uuid;

use crate::{harness_connect, now_ms, ui, Config, RawModeGuard};

const RESUME_CONTEXT_MAX_CHARS: usize = 200_000;
// Linux rejects a single exec argument around 128 KiB even when ARG_MAX is
// larger. Keep enough headroom for the harness flags and multibyte text.
const RESUME_PROMPT_MAX_BYTES: usize = 96 * 1024;
const RESUME_HARNESSES: &[&str] = &["pi", "claude", "codex"];
const FORK_DISCOVERY_LIMIT: usize = 100;

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
}

pub(crate) async fn resume_cmd(
    config: &Config,
    session_id: &str,
    requested_harness: Option<&str>,
    source_harness: Option<&str>,
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
    let plan = build_launch_plan(config, &target, &source, &directory.path, &prompt)?;

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

    Ok(ResumeSource {
        session_id: session_id.to_string(),
        harness,
        captures,
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
    cwd: &Path,
    prompt: &str,
) -> Result<LaunchPlan> {
    let spec = harness_connect::spec_by_name(target)
        .with_context(|| format!("unknown harness '{target}'"))?;
    let config_dir = harness_connect::resolve_config_dir(config, spec, None);
    let binary = harness_connect::resolve_harness_binary(config, spec)
        .with_context(|| format!("{target} is not installed or not on PATH"))?;
    let mut args = match target {
        "pi" => {
            let model = pi_resume_model(&config_dir, source.routed_model.as_deref())?;
            vec![
                OsString::from("--provider"),
                OsString::from("alexandria"),
                OsString::from("--model"),
                OsString::from(model),
            ]
        }
        "claude" => vec![
            OsString::from("--settings"),
            config_dir
                .join(harness_connect::CLAUDE_PROFILE_FILE)
                .into_os_string(),
        ],
        "codex" => vec![OsString::from("--profile"), OsString::from("alex")],
        _ => unreachable!("target validation restricts resume harnesses"),
    };
    args.push(OsString::from(prompt.replace('\0', "�")));
    Ok(LaunchPlan {
        harness: target.to_string(),
        binary,
        args,
        cwd: cwd.to_path_buf(),
        config_dir,
    })
}

fn pi_resume_model(config_dir: &Path, source_model: Option<&str>) -> Result<String> {
    let models = harness_connect::read_pi_model_ids(config_dir);
    if models.is_empty() {
        bail!("Pi's Alex model catalog is empty; run `alex connect pi` again");
    }
    let normalized_source = source_model.map(|model| {
        if model.starts_with("alex/") {
            model.to_string()
        } else {
            format!("alex/{model}")
        }
    });
    if let Some(model) = normalized_source.filter(|model| models.contains(model)) {
        return Ok(model);
    }
    let settings_path = config_dir.join("settings.json");
    if let Ok(raw) = std::fs::read_to_string(settings_path) {
        if let Ok(settings) = serde_json::from_str::<Value>(&raw) {
            if settings["defaultProvider"].as_str() == Some("alexandria") {
                if let Some(model) = settings["defaultModel"].as_str() {
                    let model = if model.starts_with("alex/") {
                        model.to_string()
                    } else {
                        format!("alex/{model}")
                    };
                    if models.contains(&model) {
                        return Ok(model);
                    }
                }
            }
        }
    }
    Ok(models[0].clone())
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
    let status = loop {
        if !recorded {
            if let Some(target_session_id) =
                find_fork_target(store, &plan.harness, token, started_ms)?
            {
                store.record_session_fork(&SessionForkRecord {
                    source_session_id: source.session_id.clone(),
                    source_harness: source.harness.clone(),
                    target_session_id: target_session_id.clone(),
                    target_harness: plan.harness.clone(),
                    created_ms: now_ms(),
                    recovered_cwd: directory
                        .evidence
                        .as_ref()
                        .map(|_| directory.path.to_string_lossy().into_owned()),
                })?;
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
                store.record_session_fork(&SessionForkRecord {
                    source_session_id: source.session_id.clone(),
                    source_harness: source.harness.clone(),
                    target_session_id,
                    target_harness: plan.harness.clone(),
                    created_ms: now_ms(),
                    recovered_cwd: directory
                        .evidence
                        .as_ref()
                        .map(|_| directory.path.to_string_lossy().into_owned()),
                })?;
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
            "--dry-run",
        ])
        .unwrap();
        match cli.command.unwrap() {
            crate::Command::Resume {
                session,
                harness,
                source_harness,
                dry_run,
            } => {
                assert_eq!(session, "session-1");
                assert_eq!(harness.as_deref(), Some("pi"));
                assert_eq!(source_harness.as_deref(), Some("codex"));
                assert!(dry_run);
            }
            _ => panic!("unexpected command"),
        }
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
