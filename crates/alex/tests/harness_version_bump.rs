// The bump script is bash; Windows cannot exec .sh files directly.
#![cfg(unix)]

use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn tmpdir(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "alex-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn bump_harness_versions_check_and_write_use_registry_payloads() {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = manifest
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("scripts/bump-harness-versions.sh");
    let dir = tmpdir("bump-harness-versions");
    let config = dir.join("harnesses.json");
    let registry = dir.join("registry");
    fs::create_dir_all(&registry).unwrap();

    let original = r#"{
  "version": 1,
  "harnesses": {
    "codex": {
      "aliases": [],
      "default_model": "gpt-5.5",
      "default_version": "1.0.0",
      "source": {
        "mode": "registered source checkout",
        "package": "@openai/codex",
        "sentinel": "keep-me"
      }
    },
    "local-only": {
      "default_version": "local",
      "source": {
        "mode": "mounted local source"
      }
    }
  }
}
"#;
    fs::write(&config, original).unwrap();
    fs::write(
        registry.join("@openai%2Fcodex.json"),
        r#"{"name":"@openai/codex","version":"9.9.9"}"#,
    )
    .unwrap();

    let check = Command::new(&script)
        .arg("--check")
        .env("ALEX_BUMP_HARNESS_CONFIG", &config)
        .env("ALEX_BUMP_REGISTRY_DIR", &registry)
        .output()
        .unwrap();
    assert_eq!(check.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&check.stdout);
    assert!(stdout.contains("codex"));
    assert!(stdout.contains("1.0.0"));
    assert!(stdout.contains("9.9.9"));
    assert!(stdout.contains("local-only"));
    assert!(stdout.contains("no package source"));

    let write = Command::new(&script)
        .env("ALEX_BUMP_HARNESS_CONFIG", &config)
        .env("ALEX_BUMP_REGISTRY_DIR", &registry)
        .output()
        .unwrap();
    assert!(
        write.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&write.stdout),
        String::from_utf8_lossy(&write.stderr)
    );

    let updated = fs::read_to_string(&config).unwrap();
    assert!(updated.contains(r#""default_version": "9.9.9""#));
    assert!(updated.contains(r#""sentinel": "keep-me""#));
    assert_eq!(updated.replacen("9.9.9", "1.0.0", 1), original);
}
