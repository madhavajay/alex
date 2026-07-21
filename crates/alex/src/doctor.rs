use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

use crate::{
    alexandria_home, detect_service_state, installed_binaries, open_vault,
    resolve_dario_claude_bin, service_managed, service_state_label, status::status_summary, Config,
    ServiceState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CheckStatus {
    Pass,
    Warning,
    Fail,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DoctorCheck {
    pub id: &'static str,
    pub category: &'static str,
    pub status: CheckStatus,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DoctorTotals {
    pub passed: usize,
    pub warnings: usize,
    pub failed: usize,
    pub informational: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DoctorReport {
    pub version: String,
    pub healthy: bool,
    pub totals: DoctorTotals,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    fn from_checks(checks: Vec<DoctorCheck>) -> Self {
        let totals = DoctorTotals {
            passed: checks
                .iter()
                .filter(|check| check.status == CheckStatus::Pass)
                .count(),
            warnings: checks
                .iter()
                .filter(|check| check.status == CheckStatus::Warning)
                .count(),
            failed: checks
                .iter()
                .filter(|check| check.status == CheckStatus::Fail)
                .count(),
            informational: checks
                .iter()
                .filter(|check| check.status == CheckStatus::Info)
                .count(),
        };
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            healthy: totals.failed == 0,
            totals,
            checks,
        }
    }
}

fn check(
    id: &'static str,
    category: &'static str,
    status: CheckStatus,
    summary: impl Into<String>,
    detail: Option<String>,
    remediation: Option<&str>,
) -> DoctorCheck {
    DoctorCheck {
        id,
        category,
        status,
        summary: summary.into(),
        detail,
        remediation: remediation.map(str::to_owned),
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let executable = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join(&executable))
        .find(|candidate| candidate.is_file())
}

fn executable_checks() -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    match std::env::current_exe() {
        Ok(path) if path.is_file() => checks.push(check(
            "executable.current",
            "executables",
            CheckStatus::Pass,
            "Alex executable is available",
            Some(path.display().to_string()),
            None,
        )),
        Ok(path) => checks.push(check(
            "executable.current",
            "executables",
            CheckStatus::Fail,
            "The running Alex executable is no longer present",
            Some(path.display().to_string()),
            Some("reinstall Alex and then run `alex doctor` again"),
        )),
        Err(error) => checks.push(check(
            "executable.current",
            "executables",
            CheckStatus::Fail,
            "Alex could not resolve its running executable",
            Some(error.to_string()),
            Some("reinstall Alex and then run `alex doctor` again"),
        )),
    }

    let installed = installed_binaries();
    if installed.len() > 1 {
        checks.push(check(
            "executable.duplicates",
            "executables",
            CheckStatus::Warning,
            "Multiple Alex executables were found",
            Some(
                installed
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            Some("remove stale Alex installations or put the intended one first on PATH"),
        ));
    } else {
        checks.push(check(
            "executable.duplicates",
            "executables",
            CheckStatus::Pass,
            "No conflicting Alex installation was detected",
            installed.first().map(|path| path.display().to_string()),
            None,
        ));
    }

    let harnesses = ["claude", "codex", "pi", "kimi", "gemini", "grok", "amp"];
    let found = harnesses
        .into_iter()
        .filter_map(|name| find_on_path(name).map(|path| format!("{name}={}", path.display())))
        .collect::<Vec<_>>();
    checks.push(if found.is_empty() {
        check(
            "executable.harnesses",
            "executables",
            CheckStatus::Warning,
            "No supported harness CLI was found on PATH",
            None,
            Some("install a harness such as Pi, Claude Code, or Codex, then run `alex connect <harness>`"),
        )
    } else {
        check(
            "executable.harnesses",
            "executables",
            CheckStatus::Pass,
            format!("Detected {} supported harness executable(s)", found.len()),
            Some(found.join(", ")),
            None,
        )
    });
    checks
}

fn permission_check(path: &Path, directory: bool) -> DoctorCheck {
    let id = if directory {
        "permissions.data_dir"
    } else {
        "permissions.config"
    };
    let expected = if directory { "0700" } else { "0600" };
    let remediation = if directory {
        "restrict the Alex data directory to the current user"
    } else {
        "restrict config.toml to the current user"
    };
    let Ok(metadata) = std::fs::metadata(path) else {
        return check(
            id,
            "permissions",
            CheckStatus::Fail,
            format!("{} is missing", path.display()),
            None,
            Some("run Alex once to create its configuration, then retry"),
        );
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode() & 0o777;
        let too_open = if directory {
            mode & 0o077 != 0
        } else {
            mode & 0o177 != 0
        };
        return if too_open {
            check(
                id,
                "permissions",
                CheckStatus::Fail,
                format!("{} permissions are too broad", path.display()),
                Some(format!("mode {mode:04o}; expected {expected}")),
                Some(remediation),
            )
        } else {
            check(
                id,
                "permissions",
                CheckStatus::Pass,
                format!("{} is private to this user", path.display()),
                Some(format!("mode {mode:04o}")),
                None,
            )
        };
    }
    #[cfg(not(unix))]
    {
        let _ = (metadata, expected, remediation);
        check(
            id,
            "permissions",
            CheckStatus::Info,
            format!("{} exists", path.display()),
            Some("Access control is managed by the platform ACL".into()),
            None,
        )
    }
}

fn storage_checks(config: &Config) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    let probe = config.data_dir.join(format!(
        ".alex-doctor-write-{}-{}",
        std::process::id(),
        crate::now_ms()
    ));
    match std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe)
    {
        Ok(_) => {
            let removed = std::fs::remove_file(&probe);
            checks.push(check(
                "storage.write",
                "storage",
                if removed.is_ok() {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Warning
                },
                "Alex storage is writable",
                removed.err().map(|error| {
                    format!("write succeeded but the temporary probe could not be removed: {error}")
                }),
                None,
            ));
        }
        Err(error) => checks.push(check(
            "storage.write",
            "storage",
            CheckStatus::Fail,
            "Alex storage is not writable",
            Some(error.to_string()),
            Some("repair ownership, permissions, or available disk space for the Alex data directory"),
        )),
    }

    let database = config.data_dir.join("alexandria.sqlite3");
    if !database.exists() {
        checks.push(check(
            "storage.sqlite",
            "storage",
            CheckStatus::Warning,
            "The trace database has not been initialized",
            Some(database.display().to_string()),
            Some("start the Alex daemon once to initialize trace storage"),
        ));
        return checks;
    }
    let result = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY).and_then(
        |connection| {
            connection.query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))
        },
    );
    checks.push(match result {
        Ok(value) if value == "ok" => check(
            "storage.sqlite",
            "storage",
            CheckStatus::Pass,
            "The trace database passed SQLite quick-check",
            Some(database.display().to_string()),
            None,
        ),
        Ok(value) => check(
            "storage.sqlite",
            "storage",
            CheckStatus::Fail,
            "The trace database reported corruption",
            Some(value),
            Some("stop Alex and restore or export the trace archive before modifying the database"),
        ),
        Err(error) => check(
            "storage.sqlite",
            "storage",
            CheckStatus::Fail,
            "The trace database could not be opened read-only",
            Some(error.to_string()),
            Some("check storage permissions and whether another process holds an exclusive lock"),
        ),
    });
    checks
}

fn service_check(service: &ServiceState, daemon_up: bool) -> DoctorCheck {
    if service_managed(service) {
        return check(
            "service.lifecycle",
            "service",
            CheckStatus::Pass,
            "Alex is managed by the operating-system user service",
            Some(service_state_label(service).to_string()),
            None,
        );
    }
    let status = if daemon_up {
        CheckStatus::Warning
    } else {
        CheckStatus::Fail
    };
    let remediation = match service {
        ServiceState::Unsupported => {
            "start Alex with `alex daemon` and keep that process running on this platform"
        }
        ServiceState::WindowsTask {
            installed: true, ..
        } => "run `alex service restart` to start the installed Windows user task",
        _ => "run `alex service install` to install and start the user service",
    };
    check(
        "service.lifecycle",
        "service",
        status,
        if daemon_up {
            "The daemon is running outside the OS user service"
        } else {
            "The Alex user service is not running"
        },
        Some(service_state_label(service).to_string()),
        Some(remediation),
    )
}

async fn port_check(config: &Config, daemon_up: bool) -> DoctorCheck {
    if daemon_up {
        return check(
            "network.port",
            "network",
            CheckStatus::Pass,
            format!("Alex answered on {}", config.base_url()),
            None,
            None,
        );
    }
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), config.port);
    let occupied = tokio::time::timeout(
        Duration::from_millis(750),
        tokio::net::TcpStream::connect(address),
    )
    .await
    .is_ok_and(|result| result.is_ok());
    port_check_from_probe(config, address, occupied)
}

fn port_check_from_probe(config: &Config, address: SocketAddr, occupied: bool) -> DoctorCheck {
    if occupied {
        check(
            "network.port",
            "network",
            CheckStatus::Fail,
            format!(
                "Port {} is occupied by something that is not a healthy Alex daemon",
                config.port
            ),
            Some(address.to_string()),
            Some("stop the conflicting process or choose a different Alex port in config.toml"),
        )
    } else {
        check(
            "network.port",
            "network",
            CheckStatus::Warning,
            format!("Nothing is listening on Alex port {}", config.port),
            Some(address.to_string()),
            Some("start Alex with `alex service install` or `alex daemon`"),
        )
    }
}

pub(crate) async fn diagnose(config: &Config) -> DoctorReport {
    let mut checks = executable_checks();
    checks.push(permission_check(&config.data_dir, true));
    checks.push(permission_check(
        &alexandria_home().join("config.toml"),
        false,
    ));
    checks.extend(storage_checks(config));

    let status = status_summary(config).await;
    let daemon_up = status.as_ref().is_ok_and(|summary| summary.daemon_up);
    checks.push(service_check(&detect_service_state(), daemon_up));
    checks.push(port_check(config, daemon_up).await);

    let accounts = match open_vault(config) {
        Ok(vault) => match vault.list().await {
            accounts if accounts.is_empty() => {
                checks.push(check(
                    "credentials.accounts",
                    "credentials",
                    CheckStatus::Fail,
                    "No provider account is connected",
                    None,
                    Some("run onboarding or `alex auth login <provider>`"),
                ));
                Vec::new()
            }
            accounts => {
                let reauth = accounts
                    .iter()
                    .filter(|account| account.needs_reauth())
                    .count();
                checks.push(check(
                    "credentials.accounts",
                    "credentials",
                    if reauth == 0 {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Fail
                    },
                    format!("{} provider account(s) are connected", accounts.len()),
                    (reauth > 0).then(|| format!("{reauth} account(s) require re-authentication")),
                    (reauth > 0)
                        .then_some("run `alex reauth start <provider>` for each affected account"),
                ));
                accounts
            }
        },
        Err(error) => {
            checks.push(check(
                "credentials.accounts",
                "credentials",
                CheckStatus::Fail,
                "The credential vault could not be opened",
                Some(error.to_string()),
                Some("check the Alex data-directory ownership and permissions"),
            ));
            Vec::new()
        }
    };

    match &status {
        Ok(summary) if !summary.daemon_up => checks.push(check(
            "providers.health",
            "providers",
            CheckStatus::Warning,
            "Provider health cannot be checked while the daemon is down",
            None,
            Some("start the daemon and run `alex doctor` again"),
        )),
        Ok(summary) if summary.accounts.is_empty() => checks.push(check(
            "providers.health",
            "providers",
            CheckStatus::Info,
            "There are no provider health results yet",
            None,
            None,
        )),
        Ok(summary) => {
            let down = summary
                .accounts
                .iter()
                .filter(|account| account.health == "down" || account.needs_reauth)
                .count();
            let unknown = summary
                .accounts
                .iter()
                .filter(|account| account.health == "unknown" && !account.needs_reauth)
                .count();
            checks.push(check(
                "providers.health",
                "providers",
                if down > 0 {
                    CheckStatus::Fail
                } else if unknown > 0 {
                    CheckStatus::Warning
                } else {
                    CheckStatus::Pass
                },
                if down > 0 {
                    format!("{down} provider account(s) are unhealthy")
                } else if unknown > 0 {
                    format!("{unknown} provider account(s) have not completed a health check")
                } else {
                    "All connected provider accounts are healthy".into()
                },
                None,
                (down > 0)
                    .then_some("run `alex ping all`, then re-authenticate any rejected account"),
            ));
        }
        Err(error) => checks.push(check(
            "providers.health",
            "providers",
            CheckStatus::Fail,
            "Provider health aggregation failed",
            Some(error.to_string()),
            Some("run `alex status --json` for the underlying status error"),
        )),
    }

    let needs_dario = accounts
        .iter()
        .any(|account| account.provider.as_str() == "anthropic");
    match &status {
        Ok(summary) if summary.dario_ready => checks.push(check(
            "dario.state",
            "dario",
            CheckStatus::Pass,
            "Dario has a ready generation",
            None,
            None,
        )),
        Ok(summary) if needs_dario => checks.push(check(
            "dario.state",
            "dario",
            CheckStatus::Fail,
            "Dario is not ready for the connected Claude subscription",
            summary
                .dario_response
                .as_ref()
                .map(|(status, _)| format!("admin status HTTP {status}")),
            Some("run `alex dario fix`, then `alex dario status`"),
        )),
        Err(error) if needs_dario => checks.push(check(
            "dario.state",
            "dario",
            CheckStatus::Fail,
            "Dario state could not be determined",
            Some(error.to_string()),
            Some("start the daemon and run `alex dario fix`"),
        )),
        _ => checks.push(check(
            "dario.state",
            "dario",
            CheckStatus::Info,
            "Dario is not required until a Claude subscription is connected",
            None,
            None,
        )),
    }

    let node = crate::dario::resolve_dario_node_bin(config.dario_node_path.as_deref());
    let claude = resolve_dario_claude_bin(config.dario_claude_bin.as_deref());
    checks.push(check(
        "dario.executables",
        "dario",
        if !needs_dario || (node.is_some() && claude.is_some()) {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        if node.is_some() && claude.is_some() {
            "Dario runtime executables are available"
        } else if needs_dario {
            "Dario is missing Node.js or Claude Code"
        } else {
            "Dario runtime executables are optional for the current providers"
        },
        Some(format!(
            "node={}, claude={}",
            node.as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "not found".into()),
            claude
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "not found".into())
        )),
        (needs_dario && (node.is_none() || claude.is_none()))
            .then_some("install Node.js 18+ and Claude Code, then run `alex dario fix`"),
    ));

    DoctorReport::from_checks(checks)
}

pub(crate) fn print_report(report: &DoctorReport, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!("Alex doctor v{}", report.version);
    println!();
    for check in &report.checks {
        let marker = match check.status {
            CheckStatus::Pass => "PASS",
            CheckStatus::Warning => "WARN",
            CheckStatus::Fail => "FAIL",
            CheckStatus::Info => "INFO",
        };
        println!("[{marker}] {:<13} {}", check.category, check.summary);
        if let Some(detail) = &check.detail {
            println!("       {detail}");
        }
        if let Some(remediation) = &check.remediation {
            println!("       fix: {remediation}");
        }
    }
    println!();
    println!(
        "{} passed · {} warnings · {} failed · {} informational",
        report.totals.passed,
        report.totals.warnings,
        report.totals.failed,
        report.totals.informational
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "alex-doctor-{name}-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn report_health_tracks_failures_not_warnings() {
        let warning = check(
            "test.warning",
            "test",
            CheckStatus::Warning,
            "warning",
            None,
            None,
        );
        let report = DoctorReport::from_checks(vec![warning]);
        assert!(report.healthy);
        assert_eq!(report.totals.warnings, 1);

        let failed = check(
            "test.failure",
            "test",
            CheckStatus::Fail,
            "failure",
            None,
            None,
        );
        let report = DoctorReport::from_checks(vec![failed]);
        assert!(!report.healthy);
        assert_eq!(report.totals.failed, 1);
    }

    #[test]
    fn storage_check_is_bounded_and_does_not_leave_probe_files() {
        let root = temp_dir("storage");
        let config = Config::defaults_for(root.clone());
        let checks = storage_checks(&config);
        assert!(checks
            .iter()
            .any(|check| { check.id == "storage.write" && check.status == CheckStatus::Pass }));
        assert!(!std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".alex-doctor-write")));
    }

    #[test]
    fn sqlite_check_detects_a_valid_store_without_reading_trace_bodies() {
        let root = temp_dir("sqlite");
        let _store = alex_store::Store::open(root.clone()).unwrap();
        let config = Config::defaults_for(root);
        let checks = storage_checks(&config);
        assert!(checks
            .iter()
            .any(|check| { check.id == "storage.sqlite" && check.status == CheckStatus::Pass }));
    }

    #[test]
    fn port_check_distinguishes_an_unused_port_from_a_non_alex_listener() {
        let mut config = Config::defaults_for(temp_dir("port"));
        config.port = 41_000;
        let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), config.port);

        let occupied = port_check_from_probe(&config, address, true);
        assert_eq!(occupied.status, CheckStatus::Fail);
        assert!(occupied.summary.contains("occupied"));

        let unused = port_check_from_probe(&config, address, false);
        assert_eq!(unused.status, CheckStatus::Warning);
        assert!(unused.summary.contains("Nothing is listening"));
    }
}
