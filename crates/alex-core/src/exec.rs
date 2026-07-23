//! Cross-platform executable resolution and atomic file replacement.
//!
//! Windows needs three things Unix code tends to get wrong:
//! - executables carry `PATHEXT` extensions (`node.exe`, `npm.cmd`), so a
//!   bare-name existence probe never matches;
//! - `.cmd`/`.bat` shims (every npm-installed CLI) cannot be spawned via
//!   `CreateProcess` directly — they need `cmd /C`;
//! - renaming over an open destination fails with a sharing violation, so
//!   atomic replaces remove the destination first.

use std::path::{Path, PathBuf};

/// Candidate filenames for an executable `bin` inside `dir`.
///
/// On Windows this is the `PATHEXT` extensions (falling back to
/// `.EXE;.CMD;.BAT`) — the bare name is excluded because `CreateProcess`
/// cannot run extensionless files, and node/npm installs ship extensionless
/// sh shims alongside the real `.cmd` ones. Elsewhere it is just `dir/bin`.
pub fn executable_candidates(dir: &Path, bin: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let pathext = std::env::var_os("PATHEXT")
            .unwrap_or_else(|| std::ffi::OsString::from(".EXE;.CMD;.BAT"));
        let mut out = Vec::new();
        for ext in pathext.to_string_lossy().split(';') {
            if ext.is_empty() {
                continue;
            }
            out.push(dir.join(format!("{bin}{ext}")));
        }
        out
    }
    #[cfg(not(windows))]
    {
        vec![dir.join(bin)]
    }
}

/// Whether `path` is a file the current platform will treat as executable.
pub fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Whether spawning `program` requires a `cmd /C` wrapper on Windows.
///
/// `CreateProcess` runs `.exe`/`.com` images directly but cannot execute
/// `.cmd`/`.bat` shims (the form npm installs CLIs in).
pub fn needs_cmd_wrapper(program: &Path) -> bool {
    if !cfg!(windows) {
        return false;
    }
    program
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
}

/// A `Command` for `program` that survives `.cmd`/`.bat` shims on Windows.
pub fn command_for(program: &Path) -> std::process::Command {
    if needs_cmd_wrapper(program) {
        let mut command = std::process::Command::new("cmd");
        command.arg("/C").arg(program);
        command
    } else {
        std::process::Command::new(program)
    }
}

/// Find `bin` in the directories of `PATH`, honoring `PATHEXT` on Windows.
///
/// `dir_filter` lets callers skip directories (e.g. macOS TCC-protected
/// folders); pass `|_| true` for no filtering.
pub fn find_on_path_filtered(bin: &str, dir_filter: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if !dir_filter(&dir) {
            continue;
        }
        for candidate in executable_candidates(&dir, bin) {
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Platform-appropriate filename for a generated hook/shim script: `.ps1`
/// on Windows (PowerShell refuses `-File` arguments without it), `.sh`
/// elsewhere.
pub fn hook_file_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.ps1")
    } else {
        format!("{stem}.sh")
    }
}

/// Rename `source` over `destination`, replacing it if present.
///
/// Windows cannot rename over an existing file that is open and mapped, and
/// historically not at all; removing the destination first matches the
/// pattern used across Alex and keeps the window of non-existence minimal.
pub fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        if destination.exists() {
            std::fs::remove_file(destination)?;
        }
    }
    std::fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "alex-exec-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn candidates_include_bare_name_on_unix() {
        let dir = Path::new("x");
        let candidates = executable_candidates(dir, "node");
        if cfg!(windows) {
            assert!(!candidates.contains(&dir.join("node")), "{candidates:?}");
        } else {
            assert_eq!(candidates[0], dir.join("node"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn candidates_include_pathext_variants_on_windows() {
        let dir = Path::new("x");
        let names: Vec<String> = executable_candidates(dir, "node")
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_lowercase())
            .collect();
        assert!(names.contains(&"node.exe".to_string()), "{names:?}");
        assert!(names.contains(&"node.cmd".to_string()), "{names:?}");
    }

    #[cfg(not(windows))]
    #[test]
    fn candidates_are_bare_only_on_unix() {
        assert_eq!(executable_candidates(Path::new("x"), "node").len(), 1);
    }

    #[test]
    fn cmd_wrapper_only_for_windows_shims() {
        assert!(!needs_cmd_wrapper(Path::new("/usr/bin/node")));
        if cfg!(windows) {
            assert!(needs_cmd_wrapper(Path::new("C:\\npm\\npm.cmd")));
            assert!(needs_cmd_wrapper(Path::new("C:\\npm\\run.BAT")));
            assert!(!needs_cmd_wrapper(Path::new("C:\\nodejs\\node.exe")));
        } else {
            assert!(!needs_cmd_wrapper(Path::new("npm.cmd")));
        }
    }

    #[test]
    fn hook_file_name_matches_platform_interpreter() {
        let name = hook_file_name("alex-session-hook");
        if cfg!(windows) {
            assert_eq!(name, "alex-session-hook.ps1");
        } else {
            assert_eq!(name, "alex-session-hook.sh");
        }
    }

    #[test]
    fn atomic_replace_overwrites_existing_destination() {
        let dir = temp_dir("replace");
        let source = dir.join("new.txt");
        let destination = dir.join("dest.txt");
        std::fs::write(&source, "new").unwrap();
        std::fs::write(&destination, "old").unwrap();
        atomic_replace(&source, &destination).unwrap();
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "new");
        assert!(!source.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_replace_works_without_existing_destination() {
        let dir = temp_dir("fresh");
        let source = dir.join("new.txt");
        let destination = dir.join("dest.txt");
        std::fs::write(&source, "new").unwrap();
        atomic_replace(&source, &destination).unwrap();
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_on_path_filtered_discovers_executables() {
        let dir = temp_dir("path");
        let name = if cfg!(windows) { "probe-bin.cmd" } else { "probe-bin" };
        let bin = dir.join(name);
        std::fs::write(&bin, "echo ok").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let original = std::env::var_os("PATH");
        let joined = std::env::join_paths(
            std::iter::once(dir.clone()).chain(
                original
                    .as_ref()
                    .map(|p| std::env::split_paths(p).collect::<Vec<_>>())
                    .unwrap_or_default(),
            ),
        )
        .unwrap();
        std::env::set_var("PATH", &joined);
        let found = find_on_path_filtered("probe-bin", |_| true);
        if let Some(path) = original {
            std::env::set_var("PATH", path);
        }
        // NTFS is case-insensitive: the PATHEXT-derived candidate may differ
        // from the on-disk spelling only by extension case.
        let found = found.expect("probe-bin should be discovered on PATH");
        assert_eq!(
            found.to_string_lossy().to_lowercase(),
            bin.to_string_lossy().to_lowercase()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
