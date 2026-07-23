use anyhow::Result;

#[cfg(not(target_os = "linux"))]
use anyhow::bail;

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::{File, OpenOptions};
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::time::Duration;

    use anyhow::{bail, Context, Result};
    use ksni::menu::{StandardItem, SubMenu};
    use ksni::{Category, Icon, MenuItem, Status, ToolTip, TrayMethods};
    use serde_json::Value;
    use tokio::sync::mpsc::{self, UnboundedSender};

    use super::super::Config;

    const AUTOSTART_NAME: &str = "alex-tray.desktop";
    const REFRESH_SECONDS: u64 = 15;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Health {
        Healthy,
        Warning,
        Offline,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct AccountSummary {
        provider: String,
        label: String,
        health: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Snapshot {
        health: Health,
        version: Option<String>,
        accounts: Vec<AccountSummary>,
    }

    impl Snapshot {
        fn offline() -> Self {
            Self {
                health: Health::Offline,
                version: None,
                accounts: Vec::new(),
            }
        }

        fn healthy_accounts(&self) -> usize {
            self.accounts
                .iter()
                .filter(|account| account.health == "healthy")
                .count()
        }

        fn status_line(&self) -> String {
            match self.health {
                Health::Offline => "Daemon offline".into(),
                Health::Healthy if self.accounts.is_empty() => "Online · no accounts".into(),
                Health::Healthy | Health::Warning => format!(
                    "Online · {}/{} accounts healthy",
                    self.healthy_accounts(),
                    self.accounts.len()
                ),
            }
        }
    }

    #[derive(Debug)]
    enum Action {
        Open(String),
        Refresh,
        Restart,
        Quit,
    }

    #[derive(Debug)]
    struct AlexTray {
        snapshot: Snapshot,
        actions: UnboundedSender<Action>,
    }

    impl AlexTray {
        fn open_item(&self, label: &str, fragment: &str, icon_name: &str) -> MenuItem<Self> {
            let actions = self.actions.clone();
            let fragment = fragment.to_string();
            StandardItem {
                label: label.into(),
                icon_name: icon_name.into(),
                activate: Box::new(move |_| {
                    let _ = actions.send(Action::Open(fragment.clone()));
                }),
                ..Default::default()
            }
            .into()
        }

        fn account_menu(&self) -> MenuItem<Self> {
            let submenu = if self.snapshot.accounts.is_empty() {
                vec![StandardItem {
                    label: "No providers connected".into(),
                    enabled: false,
                    ..Default::default()
                }
                .into()]
            } else {
                self.snapshot
                    .accounts
                    .iter()
                    .map(|account| {
                        let (marker, icon_name) = match account.health.as_str() {
                            "healthy" => ("●", "emblem-ok-symbolic"),
                            "auth_failed" => ("●", "dialog-error-symbolic"),
                            "unreachable" => ("●", "network-offline-symbolic"),
                            _ => ("○", "dialog-question-symbolic"),
                        };
                        StandardItem {
                            label: format!(
                                "{marker} {} — {}",
                                display_provider(&account.provider),
                                account.label
                            ),
                            enabled: false,
                            icon_name: icon_name.into(),
                            ..Default::default()
                        }
                        .into()
                    })
                    .collect()
            };
            SubMenu {
                label: format!("Providers ({})", self.snapshot.accounts.len()),
                icon_name: "system-users-symbolic".into(),
                submenu,
                ..Default::default()
            }
            .into()
        }
    }

    impl ksni::Tray for AlexTray {
        fn id(&self) -> String {
            "alex".into()
        }

        fn category(&self) -> Category {
            Category::SystemServices
        }

        fn title(&self) -> String {
            format!("Alex — {}", self.snapshot.status_line())
        }

        fn status(&self) -> Status {
            match self.snapshot.health {
                Health::Healthy => Status::Active,
                Health::Warning | Health::Offline => Status::NeedsAttention,
            }
        }

        fn icon_name(&self) -> String {
            match self.snapshot.health {
                Health::Healthy => "network-transmit-receive-symbolic",
                Health::Warning => "dialog-warning-symbolic",
                Health::Offline => "network-offline-symbolic",
            }
            .into()
        }

        fn icon_pixmap(&self) -> Vec<Icon> {
            vec![status_icon(self.snapshot.health)]
        }

        fn attention_icon_name(&self) -> String {
            match self.snapshot.health {
                Health::Offline => "network-offline-symbolic",
                _ => "dialog-warning-symbolic",
            }
            .into()
        }

        fn attention_icon_pixmap(&self) -> Vec<Icon> {
            vec![status_icon(self.snapshot.health)]
        }

        fn tool_tip(&self) -> ToolTip {
            ToolTip {
                title: "Alex".into(),
                description: self.snapshot.status_line(),
                icon_name: self.icon_name(),
                icon_pixmap: self.icon_pixmap(),
            }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            let _ = self.actions.send(Action::Open(String::new()));
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            let status_icon = match self.snapshot.health {
                Health::Healthy => "emblem-ok-symbolic",
                Health::Warning => "dialog-warning-symbolic",
                Health::Offline => "network-offline-symbolic",
            };
            let mut items = vec![StandardItem {
                label: self.snapshot.status_line(),
                enabled: false,
                icon_name: status_icon.into(),
                ..Default::default()
            }
            .into()];
            if let Some(version) = &self.snapshot.version {
                items.push(
                    StandardItem {
                        label: format!("Alex {version}"),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
            }
            items.extend([
                self.account_menu(),
                MenuItem::Separator,
                self.open_item("Open Web UI", "", "web-browser-symbolic"),
                self.open_item("Add credentials", "onboarding", "list-add-symbolic"),
                self.open_item("View traces", "traces", "view-list-symbolic"),
                MenuItem::Separator,
            ]);
            let refresh = self.actions.clone();
            items.push(
                StandardItem {
                    label: "Refresh status".into(),
                    icon_name: "view-refresh-symbolic".into(),
                    activate: Box::new(move |_| {
                        let _ = refresh.send(Action::Refresh);
                    }),
                    ..Default::default()
                }
                .into(),
            );
            let restart = self.actions.clone();
            items.push(
                StandardItem {
                    label: "Restart daemon".into(),
                    icon_name: "system-reboot-symbolic".into(),
                    activate: Box::new(move |_| {
                        let _ = restart.send(Action::Restart);
                    }),
                    ..Default::default()
                }
                .into(),
            );
            items.push(MenuItem::Separator);
            let quit = self.actions.clone();
            items.push(
                StandardItem {
                    label: "Quit Alex tray".into(),
                    icon_name: "application-exit-symbolic".into(),
                    activate: Box::new(move |_| {
                        let _ = quit.send(Action::Quit);
                    }),
                    ..Default::default()
                }
                .into(),
            );
            items
        }
    }

    struct InstanceLock {
        _file: File,
    }

    impl InstanceLock {
        fn acquire(path: &Path) -> Result<Self> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false)
                .open(path)
                .with_context(|| format!("opening tray lock {}", path.display()))?;
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc != 0 {
                bail!("the Alex tray is already running in this user session");
            }
            file.set_len(0)?;
            writeln!(file, "{}", std::process::id())?;
            Ok(Self { _file: file })
        }
    }

    pub async fn run(config: &Config) -> Result<()> {
        let _instance = InstanceLock::acquire(&config.data_dir.join("tray.lock"))?;
        if !super::super::daemon_healthy(&config.base_url()).await {
            super::super::daemon_background(&config.host, config.port, None, None).await?;
        }

        let (actions, mut action_rx) = mpsc::unbounded_channel();
        let snapshot = fetch_snapshot(config).await;
        let tray = AlexTray { snapshot, actions };
        let handle =
            tray.assume_sni_available(true).spawn().await.context(
                "starting Linux StatusNotifierItem tray; is the session D-Bus available?",
            )?;
        eprintln!("Alex tray is running; press Ctrl-C or choose ‘Quit Alex tray’ to stop it");

        let mut refresh = tokio::time::interval(Duration::from_secs(REFRESH_SECONDS));
        refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        refresh.tick().await;

        loop {
            tokio::select! {
                _ = refresh.tick() => update_snapshot(&handle, config).await,
                _ = tokio::signal::ctrl_c() => break,
                action = action_rx.recv() => match action {
                    Some(Action::Open(fragment)) => open_web(config, &fragment),
                    Some(Action::Refresh) => update_snapshot(&handle, config).await,
                    Some(Action::Restart) => {
                        restart_daemon(config).await;
                        update_snapshot(&handle, config).await;
                    }
                    Some(Action::Quit) | None => break,
                }
            }
        }
        handle.shutdown().await;
        Ok(())
    }

    async fn update_snapshot(handle: &ksni::Handle<AlexTray>, config: &Config) {
        let snapshot = fetch_snapshot(config).await;
        let _ = handle.update(move |tray| tray.snapshot = snapshot).await;
    }

    async fn fetch_snapshot(config: &Config) -> Snapshot {
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(4))
            .build()
        else {
            return Snapshot::offline();
        };
        let base = config.base_url();
        let health = match client
            .get(format!("{base}/health"))
            .header("x-api-key", &config.local_key)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                response.json::<Value>().await.unwrap_or(Value::Null)
            }
            _ => return Snapshot::offline(),
        };
        let accounts_json = match client
            .get(format!("{base}/admin/accounts"))
            .header("x-api-key", &config.local_key)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                response.json::<Value>().await.unwrap_or(Value::Null)
            }
            _ => Value::Null,
        };
        snapshot_from_values(&health, &accounts_json)
    }

    fn snapshot_from_values(daemon_health: &Value, accounts_json: &Value) -> Snapshot {
        let mut accounts = accounts_json["accounts"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|account| AccountSummary {
                provider: account["provider"]
                    .as_str()
                    .unwrap_or("unknown")
                    .to_string(),
                label: account["email"]
                    .as_str()
                    .or_else(|| account["label"].as_str())
                    .or_else(|| account["name"].as_str())
                    .unwrap_or("configured")
                    .to_string(),
                health: account["health"]
                    .as_str()
                    .or_else(|| account["status"].as_str())
                    .unwrap_or("unknown")
                    .to_string(),
            })
            .collect::<Vec<_>>();
        accounts.sort_by(|a, b| (&a.provider, &a.label).cmp(&(&b.provider, &b.label)));
        let health = if !accounts_json["accounts"].is_array() {
            Health::Warning
        } else if accounts.iter().all(|account| account.health == "healthy") {
            Health::Healthy
        } else {
            Health::Warning
        };
        Snapshot {
            health,
            version: daemon_health["version"].as_str().map(str::to_string),
            accounts,
        }
    }

    async fn restart_daemon(config: &Config) {
        if let Err(error) = super::super::service_restart(config, false).await {
            eprintln!("could not restart Alex daemon: {error:#}");
        }
    }

    fn open_web(config: &Config, fragment: &str) {
        let base = format!("{}/ui/", config.base_url().trim_end_matches('/'));
        let url = if fragment.is_empty() {
            base
        } else {
            format!("{base}#{fragment}")
        };
        if let Err(error) = super::super::launch_browser(&url) {
            eprintln!("could not open Alex web UI: {error:#}");
        }
    }

    fn display_provider(provider: &str) -> &str {
        match provider {
            "anthropic" => "Anthropic",
            "openai" => "OpenAI",
            "gemini" => "Gemini",
            "xai" => "xAI",
            "openrouter" => "OpenRouter",
            "cliproxyapi" => "CLIProxyAPI",
            "kimi" => "Kimi",
            "amp" => "Amp",
            "exo" => "Exo",
            other => other,
        }
    }

    fn status_icon(health: Health) -> Icon {
        const SIZE: i32 = 32;
        let mut data = vec![0_u8; (SIZE * SIZE * 4) as usize];
        let blue = [255, 46, 111, 179];
        let white = [255, 255, 255, 255];
        let badge = match health {
            Health::Healthy => [255, 61, 220, 132],
            Health::Warning => [255, 255, 184, 77],
            Health::Offline => [255, 255, 100, 112],
        };
        for y in 0..SIZE {
            for x in 0..SIZE {
                let dx = x - 15;
                let dy = y - 15;
                if dx * dx + dy * dy <= 14 * 14 {
                    put_pixel(&mut data, SIZE, x, y, blue);
                }
                let left = (x - 9).abs() <= 1 && (10..=22).contains(&y);
                let right = (x - 20).abs() <= 1 && (10..=22).contains(&y);
                let top = (10..=20).contains(&x) && (y - (9 + (x - 15).abs())).abs() <= 1;
                let bar = (11..=19).contains(&x) && (15..=17).contains(&y);
                if left || right || top || bar {
                    put_pixel(&mut data, SIZE, x, y, white);
                }
            }
        }
        for y in 21..SIZE {
            for x in 21..SIZE {
                let dx = x - 26;
                let dy = y - 26;
                if dx * dx + dy * dy <= 5 * 5 {
                    put_pixel(&mut data, SIZE, x, y, badge);
                }
            }
        }
        Icon {
            width: SIZE,
            height: SIZE,
            data,
        }
    }

    fn put_pixel(data: &mut [u8], width: i32, x: i32, y: i32, argb: [u8; 4]) {
        let offset = ((y * width + x) * 4) as usize;
        data[offset..offset + 4].copy_from_slice(&argb);
    }

    fn autostart_path() -> Result<PathBuf> {
        Ok(dirs::home_dir()
            .context("no home directory")?
            .join(".config/autostart")
            .join(AUTOSTART_NAME))
    }

    fn desktop_quote(value: &str) -> String {
        let escaped = value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$");
        format!("\"{escaped}\"")
    }

    fn render_autostart(executable: &Path, alex_home: Option<&Path>) -> String {
        let executable = executable.to_string_lossy();
        let exec = match alex_home {
            Some(home) => format!(
                "env {} {} tray",
                desktop_quote(&format!("ALEX_HOME={}", home.display())),
                desktop_quote(&executable)
            ),
            None => format!("{} tray", desktop_quote(&executable)),
        };
        format!(
            "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Alex Status\nComment=Alex daemon and provider status\nExec={exec}\nTryExec={}\nIcon=network-transmit-receive\nTerminal=false\nStartupNotify=false\nX-GNOME-Autostart-enabled=true\nX-GNOME-Autostart-Delay=5\n",
            desktop_quote(&executable)
        )
    }

    pub fn install() -> Result<()> {
        let executable = std::env::current_exe()?
            .canonicalize()
            .context("resolving the Alex executable")?;
        let destination = autostart_path()?;
        std::fs::create_dir_all(destination.parent().expect("autostart parent"))?;
        let alex_home = std::env::var_os("ALEX_HOME").map(PathBuf::from);
        std::fs::write(
            &destination,
            render_autostart(&executable, alex_home.as_deref()),
        )?;
        println!(
            "Linux tray autostart installed at {}",
            destination.display()
        );

        let graphical =
            std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
        if graphical && std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() {
            let mut command = std::process::Command::new(&executable);
            command
                .arg("tray")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Some(home) = alex_home {
                command.env("ALEX_HOME", home);
            }
            command.spawn().context("starting the Alex tray")?;
            println!("Linux tray started in this desktop session");
        } else {
            println!("The tray will start at the next graphical login");
        }
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        let destination = autostart_path()?;
        if destination.exists() {
            std::fs::remove_file(&destination)?;
            println!(
                "Linux tray autostart removed from {}",
                destination.display()
            );
        } else {
            println!("Linux tray autostart is not installed");
        }
        Ok(())
    }

    pub fn status() -> Result<()> {
        let destination = autostart_path()?;
        if destination.is_file() {
            println!(
                "Linux tray autostart: installed ({})",
                destination.display()
            );
        } else {
            println!("Linux tray autostart: not installed");
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn snapshot_uses_reachability_and_sorts_accounts() {
            let snapshot = snapshot_from_values(
                &serde_json::json!({"version": "1.2.3"}),
                &serde_json::json!({"accounts": [
                    {"provider":"openai","email":"z@example.com","status":"active","health":"healthy"},
                    {"provider":"anthropic","label":"Claude","status":"active","health":"auth_failed"}
                ]}),
            );
            assert_eq!(snapshot.health, Health::Warning);
            assert_eq!(snapshot.version.as_deref(), Some("1.2.3"));
            assert_eq!(snapshot.accounts[0].provider, "anthropic");
            assert_eq!(snapshot.status_line(), "Online · 1/2 accounts healthy");
        }

        #[test]
        fn empty_online_snapshot_is_healthy() {
            let snapshot = snapshot_from_values(
                &serde_json::json!({"version": "1.2.3"}),
                &serde_json::json!({"accounts": []}),
            );
            assert_eq!(snapshot.health, Health::Healthy);
            assert_eq!(snapshot.status_line(), "Online · no accounts");
        }

        #[test]
        fn generated_icon_is_complete_argb() {
            for health in [Health::Healthy, Health::Warning, Health::Offline] {
                let icon = status_icon(health);
                assert_eq!((icon.width, icon.height), (32, 32));
                assert_eq!(icon.data.len(), 32 * 32 * 4);
                assert!(icon.data.chunks_exact(4).any(|pixel| pixel[0] != 0));
            }
        }

        #[test]
        fn autostart_preserves_paths_without_using_a_shell() {
            let entry = render_autostart(
                Path::new("/tmp/Alex Build/alex"),
                Some(Path::new("/tmp/Alex Home")),
            );
            assert!(entry
                .contains("Exec=env \"ALEX_HOME=/tmp/Alex Home\" \"/tmp/Alex Build/alex\" tray"));
            assert!(entry.contains("TryExec=\"/tmp/Alex Build/alex\""));
            assert!(!entry.contains("sh -c"));
        }
    }
}

#[cfg(target_os = "linux")]
pub async fn run(config: &super::Config) -> Result<()> {
    linux::run(config).await
}

#[cfg(not(target_os = "linux"))]
pub async fn run(_config: &super::Config) -> Result<()> {
    bail!("the desktop status tray is currently available on Linux; macOS uses the menu-bar app")
}

#[cfg(target_os = "linux")]
pub fn install() -> Result<()> {
    linux::install()
}

#[cfg(not(target_os = "linux"))]
pub fn install() -> Result<()> {
    bail!("tray autostart is currently available on Linux")
}

#[cfg(target_os = "linux")]
pub fn uninstall() -> Result<()> {
    linux::uninstall()
}

#[cfg(not(target_os = "linux"))]
pub fn uninstall() -> Result<()> {
    bail!("tray autostart is currently available on Linux")
}

#[cfg(target_os = "linux")]
pub fn status() -> Result<()> {
    linux::status()
}

#[cfg(not(target_os = "linux"))]
pub fn status() -> Result<()> {
    bail!("tray autostart is currently available on Linux")
}
