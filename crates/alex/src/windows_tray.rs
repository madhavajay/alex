//! Windows system tray icon for the Alex daemon.
//!
//! The daemon runs as a per-user Task Scheduler task in the interactive
//! session, so a notification-area icon is visible to the user. The icon is
//! the embedded executable icon (see build.rs); the menu offers opening the
//! local web UI. Runs on its own OS thread with a Win32 message pump so it
//! never blocks the tokio runtime; all failures are non-fatal (headless
//! sessions simply have no tray).
#![cfg(windows)]

use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};

static TRAY_STARTED: AtomicBool = AtomicBool::new(false);

const WM_APP_TRAY: u32 = 0x8000 + 1; // WM_APP + 1
const CMD_OPEN_UI: usize = 1;
const CMD_STATUS: usize = 2;
const CMD_OPEN_TRACES: usize = 3;
const CMD_CHECK_UPDATES: usize = 4;
const CMD_STAR_GITHUB: usize = 5;
const CMD_TOGGLE_STARTUP: usize = 6;
const CMD_QUIT: usize = 7;
const CMD_PING: usize = 8;
const CMD_CONNECT_SUBSCRIPTION: usize = 9;
const CMD_REAUTH: usize = 10;
const CMD_MANAGE_ACCOUNTS: usize = 11;
// Harness entries occupy CMD_HARNESS_BASE..CMD_HARNESS_BASE+len.
const CMD_HARNESS_BASE: usize = 100;

/// Harnesses offered in the tray flyout; `alex up <name>` installs and
/// connects each one interactively in a console window.
const TRAY_HARNESSES: &[&str] = &[
    "pi", "claude", "codex", "kimi", "gemini", "qwen", "goose", "opencode",
];

const GITHUB_URL: &str = "https://github.com/madhavajay/alex";

thread_local! {
    static TRAY_URL: RefCell<String> = const { RefCell::new(String::new()) };
}

fn startup_task_enabled() -> bool {
    // The AlexDaemon Task Scheduler entry has a logon trigger; enabled means
    // launches at startup. schtasks avoids a COM apartment on this
    // message-pump thread.
    std::process::Command::new("schtasks")
        .args(["/query", "/tn", "AlexDaemon", "/fo", "LIST"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            let text = String::from_utf8_lossy(&output.stdout).to_lowercase();
            !text.contains("disabled")
        })
        .unwrap_or(false)
}

fn set_startup_task_enabled(enable: bool) {
    let flag = if enable { "/enable" } else { "/disable" };
    let _ = std::process::Command::new("schtasks")
        .args(["/change", "/tn", "AlexDaemon", flag])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Run our own CLI in a visible console window so interactive flows
/// (harness install, subscription auth with a browser round-trip, ping
/// output) have somewhere to show progress and prompt the user.
fn run_own_binary_in_console(args: &[&str]) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let joined = args.join(" ");
    // raw_arg avoids std's re-quoting, which cmd's own parser mangles
    // (a quoted `start "title" ...` string shows a "cannot find '\Alex\'"
    // dialog instead of running). /K keeps the window open afterwards.
    let mut command = std::process::Command::new("cmd");
    command.raw_arg(format!("/K \"{}\" {joined}", exe.display()));
    command.creation_flags(CREATE_NEW_CONSOLE);
    let _ = command.spawn();
}

fn open_in_browser(url: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let verb = wide("open");
    let url_wide = wide(url);
    unsafe {
        ShellExecuteW(
            null_mut(),
            verb.as_ptr(),
            url_wide.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        );
    }
}

pub fn spawn(ui_url: String) {
    if TRAY_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("alex-tray".into())
        .spawn(move || {
            if let Err(error) = run_message_loop(&ui_url) {
                tracing::warn!(%error, "windows tray icon unavailable");
            }
        })
        .ok();
}

fn wide(text: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

unsafe extern "system" fn window_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    if msg != WM_APP_TRAY {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let event = (lparam as u32) & 0xFFFF;
    if event == WM_LBUTTONUP || event == WM_RBUTTONUP {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return 0;
        }
        let status_label = wide("Daemon running");
        let open_label = wide("Open Alex");
        let traces_label = wide("Open Trace Browser");
        let ping_label = wide("Ping Providers");
        let updates_label = wide("Check for Updates");
        let star_label = wide("Star on GitHub");
        let startup_label = wide("Launch at Startup");
        let quit_label = wide("Quit Alex");

        AppendMenuW(menu, MF_STRING | MF_GRAYED, CMD_STATUS, status_label.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(menu, MF_STRING, CMD_OPEN_UI, open_label.as_ptr());
        AppendMenuW(menu, MF_STRING, CMD_OPEN_TRACES, traces_label.as_ptr());
        AppendMenuW(menu, MF_STRING, CMD_PING, ping_label.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, null());

        // Harnesses flyout: install/connect each supported harness.
        let harness_menu = CreatePopupMenu();
        for (index, name) in TRAY_HARNESSES.iter().enumerate() {
            let label = wide(&format!("Install / Connect {name}"));
            AppendMenuW(
                harness_menu,
                MF_STRING,
                CMD_HARNESS_BASE + index,
                label.as_ptr(),
            );
        }
        let harness_label = wide("Harnesses");
        AppendMenuW(
            menu,
            MF_POPUP,
            harness_menu as usize,
            harness_label.as_ptr(),
        );

        // Subscriptions flyout: connect / reauth / manage accounts.
        let subs_menu = CreatePopupMenu();
        let connect_label = wide("Connect a Subscription…");
        let reauth_label = wide("Re-authenticate…");
        let accounts_label = wide("Manage Accounts (Web UI)");
        AppendMenuW(subs_menu, MF_STRING, CMD_CONNECT_SUBSCRIPTION, connect_label.as_ptr());
        AppendMenuW(subs_menu, MF_STRING, CMD_REAUTH, reauth_label.as_ptr());
        AppendMenuW(subs_menu, MF_STRING, CMD_MANAGE_ACCOUNTS, accounts_label.as_ptr());
        let subs_label = wide("Subscriptions");
        AppendMenuW(menu, MF_POPUP, subs_menu as usize, subs_label.as_ptr());

        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        let startup_flags = if startup_task_enabled() {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        AppendMenuW(menu, startup_flags, CMD_TOGGLE_STARTUP, startup_label.as_ptr());
        AppendMenuW(menu, MF_STRING, CMD_CHECK_UPDATES, updates_label.as_ptr());
        AppendMenuW(menu, MF_STRING, CMD_STAR_GITHUB, star_label.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(menu, MF_STRING, CMD_QUIT, quit_label.as_ptr());

        let mut point = POINT { x: 0, y: 0 };
        GetCursorPos(&mut point);
        SetForegroundWindow(hwnd);
        let picked = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY,
            point.x,
            point.y,
            0,
            hwnd,
            null(),
        );
        DestroyMenu(menu);
        let ui_url = TRAY_URL.with(|u| u.borrow().clone());
        let picked = picked as usize;
        match picked {
            CMD_OPEN_UI => open_in_browser(&ui_url),
            CMD_OPEN_TRACES => open_in_browser(&format!("{ui_url}#traces")),
            CMD_PING => run_own_binary_in_console(&["ping"]),
            CMD_STAR_GITHUB => open_in_browser(GITHUB_URL),
            CMD_CHECK_UPDATES => {
                run_own_binary_in_console(&["update"]);
            }
            CMD_CONNECT_SUBSCRIPTION => open_in_browser(&format!("{ui_url}#onboarding")),
            CMD_REAUTH => run_own_binary_in_console(&["reauth"]),
            CMD_MANAGE_ACCOUNTS => open_in_browser(&format!("{ui_url}#status")),
            CMD_TOGGLE_STARTUP => {
                set_startup_task_enabled(!startup_task_enabled());
            }
            CMD_QUIT => {
                // Ending the process stops the daemon; the logon-triggered
                // task brings it back at next sign-in unless startup is
                // toggled off.
                std::process::exit(0);
            }
            picked if picked >= CMD_HARNESS_BASE
                && picked < CMD_HARNESS_BASE + TRAY_HARNESSES.len() =>
            {
                let name = TRAY_HARNESSES[picked - CMD_HARNESS_BASE];
                run_own_binary_in_console(&["up", name]);
            }
            _ => {}
        }
    }
    0
}

fn run_message_loop(ui_url: &str) -> Result<(), String> {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    TRAY_URL.with(|u| *u.borrow_mut() = ui_url.to_string());

    unsafe {
        let instance = GetModuleHandleW(null());
        let class_name = wide("AlexTrayWindow");

        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class_name.as_ptr(),
        };
        if RegisterClassW(&class) == 0 {
            return Err("RegisterClassW failed".into());
        }

        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            instance,
            null(),
        );
        if window.is_null() {
            return Err("CreateWindowExW failed".into());
        }

        // Icon resource id 1 is what winresource assigns the first icon.
        let icon = LoadIconW(instance, 1 as *const u16);

        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = window;
        data.uID = 1;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = WM_APP_TRAY;
        data.hIcon = icon;
        let tip = wide("Alex — local control plane");
        let tip_len = tip.len().min(data.szTip.len());
        data.szTip[..tip_len].copy_from_slice(&tip[..tip_len]);
        if Shell_NotifyIconW(NIM_ADD, &data) == 0 {
            return Err("Shell_NotifyIconW(NIM_ADD) failed".into());
        }

        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        Shell_NotifyIconW(NIM_DELETE, &data);
    }
    Ok(())
}
