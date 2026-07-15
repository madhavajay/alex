//! Fail-open, time-bounded JSON-lines plugin host.  Plugins are long-lived
//! child processes; a request is never allowed to wait indefinitely for one.
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub name: String,
    pub command: Vec<String>,
    pub hooks: Vec<String>,
    #[serde(default)]
    pub mutation: bool,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}
fn default_timeout_ms() -> u64 {
    150
}

impl PluginManifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.command.is_empty() {
            return Err("plugin needs name and command".into());
        }
        if self.timeout_ms == 0 || self.timeout_ms > 5_000 {
            return Err("plugin timeout_ms must be 1..5000".into());
        }
        for hook in &self.hooks {
            if !matches!(
                hook.as_str(),
                "on_request" | "on_response" | "on_tool_call" | "on_tool_result" | "on_trace"
            ) {
                return Err(format!("unsupported plugin hook '{hook}'"));
            }
        }
        Ok(())
    }
}

struct Process {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
}
impl Process {
    fn start(manifest: &PluginManifest) -> std::io::Result<Self> {
        let mut command = Command::new(&manifest.command[0]);
        command
            .args(&manifest.command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        Ok(Self {
            input: child.stdin.take().expect("piped stdin"),
            output: BufReader::new(child.stdout.take().expect("piped stdout")),
            child,
            next_id: 1,
        })
    }
    fn call(&mut self, hook: &str, payload: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        serde_json::to_writer(
            &mut self.input,
            &json!({"id": id, "hook": hook, "payload": payload}),
        )
        .map_err(|e| e.to_string())?;
        self.input.write_all(b"\n").map_err(|e| e.to_string())?;
        self.input.flush().map_err(|e| e.to_string())?;
        let mut line = String::new();
        self.output
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        let response: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        if response["id"].as_u64() != Some(id) {
            return Err("plugin response id mismatch".into());
        }
        Ok(response)
    }
}

struct Plugin {
    manifest: PluginManifest,
    process: Arc<Mutex<Option<Process>>>,
}
pub struct PluginManager {
    plugins: Vec<Plugin>,
}
impl PluginManager {
    pub fn empty() -> Self {
        Self { plugins: vec![] }
    }
    pub fn from_manifests(manifests: Vec<PluginManifest>) -> Result<Self, String> {
        for manifest in &manifests {
            manifest.validate()?;
        }
        Ok(Self {
            plugins: manifests
                .into_iter()
                .map(|manifest| Plugin {
                    manifest,
                    process: Arc::new(Mutex::new(None)),
                })
                .collect(),
        })
    }
    /// Returns the original payload on every error, timeout, exit, or invalid
    /// reply. Mutations are accepted only from manifests that explicitly ask
    /// for them and only for the pre-execution hooks.
    pub fn invoke(&self, hook: &str, payload: Value) -> Value {
        let mut current = payload;
        for plugin in &self.plugins {
            if !plugin.manifest.hooks.iter().any(|wanted| wanted == hook) {
                continue;
            }
            let candidate = current.clone();
            let timeout = Duration::from_millis(plugin.manifest.timeout_ms);
            let process = plugin.process.clone();
            let manifest = plugin.manifest.clone();
            let hook = hook.to_string();
            let worker_hook = hook.clone();
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            std::thread::spawn(move || {
                let mut slot = process.lock().unwrap_or_else(|p| p.into_inner());
                if slot.is_none() {
                    *slot = Process::start(&manifest).ok();
                }
                let _ = tx.send(
                    slot.as_mut()
                        .ok_or_else(|| "plugin failed to start".to_string())
                        .and_then(|p| p.call(&worker_hook, candidate)),
                );
            });
            let result = match rx.recv_timeout(timeout) {
                Ok(Ok(value)) => Some(value),
                _ => {
                    if let Ok(mut slot) = plugin.process.lock() {
                        if let Some(process) = slot.as_mut() {
                            let _ = process.child.kill();
                        }
                        *slot = None;
                    }
                    None
                }
            };
            let Some(response) = result else {
                continue;
            };
            if plugin.manifest.mutation && matches!(hook.as_str(), "on_request" | "on_tool_call") {
                if let Some(next) = response.get("payload").filter(|value| value.is_object()) {
                    current = next.clone();
                }
            }
        }
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest(script: &str) -> PluginManifest {
        PluginManifest {
            name: "test".into(),
            command: vec!["sh".into(), "-c".into(), script.into()],
            hooks: vec!["on_request".into()],
            mutation: true,
            timeout_ms: 30,
        }
    }
    #[test]
    fn timeout_is_fail_open() {
        let p = PluginManager::from_manifests(vec![manifest("sleep 1")]).unwrap();
        assert_eq!(p.invoke("on_request", json!({"x":1})), json!({"x":1}));
    }
    #[test]
    fn exit_is_fail_open() {
        let p = PluginManager::from_manifests(vec![manifest("exit 2")]).unwrap();
        assert_eq!(p.invoke("on_request", json!({"x":1})), json!({"x":1}));
    }
    #[test]
    fn malformed_is_fail_open() {
        let p = PluginManager::from_manifests(vec![manifest("echo nope")]).unwrap();
        assert_eq!(p.invoke("on_request", json!({"x":1})), json!({"x":1}));
    }
}
