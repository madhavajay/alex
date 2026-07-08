use std::io::IsTerminal;
use std::sync::OnceLock;

use alex_auth::now_ms;

pub fn colors_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none()
            && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true)
    })
}

pub fn paint(text: &str, code: &str) -> String {
    if colors_enabled() && !text.is_empty() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

pub fn bold(text: &str) -> String {
    paint(text, "1")
}

pub fn dim(text: &str) -> String {
    paint(text, "2")
}

pub fn red(text: &str) -> String {
    paint(text, "31")
}

pub fn green(text: &str) -> String {
    paint(text, "32")
}

pub fn yellow(text: &str) -> String {
    paint(text, "33")
}

#[allow(dead_code)]
pub fn magenta(text: &str) -> String {
    paint(text, "35")
}

pub fn cyan(text: &str) -> String {
    paint(text, "36")
}

pub fn gold(text: &str) -> String {
    paint(text, "38;5;220")
}

pub fn amber(text: &str) -> String {
    paint(text, "38;5;178")
}

pub fn sand(text: &str) -> String {
    paint(text, "38;5;180")
}

pub fn lapis(text: &str) -> String {
    paint(text, "38;5;69")
}

pub fn turquoise(text: &str) -> String {
    paint(text, "38;5;73")
}

pub fn ankh() -> &'static str {
    if colors_enabled() {
        "☥"
    } else {
        "-"
    }
}

pub fn diamond() -> &'static str {
    if colors_enabled() {
        "◆"
    } else {
        "*"
    }
}

pub fn dot() -> &'static str {
    if colors_enabled() {
        "●"
    } else {
        "*"
    }
}

pub fn selector() -> &'static str {
    if colors_enabled() {
        "❯"
    } else {
        ">"
    }
}

pub fn circle() -> &'static str {
    if colors_enabled() {
        "○"
    } else {
        "o"
    }
}

pub fn section(title: &str) -> String {
    gold(&bold(&format!("{} {title}", ankh())))
}

pub fn divider(label: &str) -> String {
    if colors_enabled() {
        gold(&format!("─── ☥ {label} ───"))
    } else {
        format!("--- {label} ---")
    }
}

pub fn column_header(text: &str) -> String {
    amber(&bold(text))
}

pub fn status_color(status: Option<i64>) -> String {
    match status {
        Some(s) if (200..300).contains(&s) => green(&s.to_string()),
        Some(s) if (300..400).contains(&s) => cyan(&s.to_string()),
        Some(s) if (400..500).contains(&s) => yellow(&s.to_string()),
        Some(s) => red(&s.to_string()),
        None => red("-"),
    }
}

pub fn gauge(pct: f64, width: usize) -> String {
    let pct = pct.clamp(0.0, 100.0);
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let bar = "█".repeat(filled);
    let rest = "░".repeat(width - filled);
    let colored = if pct < 50.0 {
        green(&bar)
    } else if pct < 80.0 {
        yellow(&bar)
    } else {
        red(&bar)
    };
    format!("{colored}{}", dim(&rest))
}

pub fn human_ms(ms: i64) -> String {
    human_secs(ms / 1000)
}

pub fn human_duration(mins: i64) -> String {
    human_secs(mins * 60)
}

fn human_secs(secs: i64) -> String {
    let s = secs.abs();
    let body = if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 48 * 3600 {
        let mins = (s % 3600) / 60;
        if mins == 0 {
            format!("{}h", s / 3600)
        } else {
            format!("{}h{:02}m", s / 3600, mins)
        }
    } else {
        format!("{}d", s / 86_400)
    };
    if secs < 0 {
        format!("-{body}")
    } else {
        body
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn human_ago(ts_ms: i64) -> String {
    let elapsed = now_ms() - ts_ms;
    if elapsed < 1000 {
        "just now".to_string()
    } else {
        format!("{} ago", human_ms(elapsed))
    }
}

pub fn visible_len(s: &str) -> usize {
    let mut len = 0;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for c2 in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c2) {
                        break;
                    }
                }
            }
        } else {
            len += 1;
        }
    }
    len
}

pub fn pad_right(s: &str, width: usize) -> String {
    let len = visible_len(s);
    if len >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - len))
    }
}

pub fn pad_left(s: &str, width: usize) -> String {
    let len = visible_len(s);
    if len >= width {
        s.to_string()
    } else {
        format!("{}{s}", " ".repeat(width - len))
    }
}

pub fn term_width() -> usize {
    crossterm::terminal::size()
        .ok()
        .map(|(w, _)| w as usize)
        .filter(|w| *w >= 40)
        .or_else(|| {
            std::env::var("COLUMNS")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|w| *w >= 40)
        })
        .unwrap_or(80)
}

pub fn clip(s: &str, max: usize) -> String {
    if visible_len(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut visible = 0usize;
    let limit = max.saturating_sub(1);
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            out.push(c);
            if let Some(b) = chars.next() {
                out.push(b);
                if b == '[' {
                    for c2 in chars.by_ref() {
                        out.push(c2);
                        if ('\x40'..='\x7e').contains(&c2) {
                            break;
                        }
                    }
                }
            }
            continue;
        }
        if visible >= limit {
            break;
        }
        out.push(c);
        visible += 1;
    }
    out.push('…');
    if colors_enabled() {
        out.push_str("\x1b[0m");
    }
    out
}

pub fn progress_bar(pct: f64, width: usize, color: &str) -> String {
    let filled = ((pct.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(width - filled));
    if colors_enabled() {
        format!("\x1b[38;5;{color}m{bar}\x1b[0m")
    } else {
        bar
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_len_strips_ansi() {
        assert_eq!(visible_len("plain"), 5);
        assert_eq!(visible_len("\x1b[32mok\x1b[0m"), 2);
        assert_eq!(visible_len("\x1b[1m\x1b[31mFAIL\x1b[0m!"), 5);
    }

    #[test]
    fn padding_is_ansi_aware() {
        let colored = "\x1b[32mok\x1b[0m";
        assert_eq!(visible_len(&pad_right(colored, 6)), 6);
        assert_eq!(visible_len(&pad_left(colored, 6)), 6);
        assert!(pad_right(colored, 6).ends_with("    "));
        assert!(pad_left(colored, 6).starts_with("    "));
        assert_eq!(pad_right("abc", 2), "abc");
    }

    fn fill_count(bar: &str) -> usize {
        bar.chars().filter(|c| *c == '█').count()
    }

    #[test]
    fn gauge_fill_boundaries() {
        assert_eq!(fill_count(&gauge(0.0, 24)), 0);
        assert_eq!(fill_count(&gauge(49.0, 24)), 12);
        assert_eq!(fill_count(&gauge(50.0, 24)), 12);
        assert_eq!(fill_count(&gauge(79.0, 24)), 19);
        assert_eq!(fill_count(&gauge(80.0, 24)), 19);
        assert_eq!(fill_count(&gauge(100.0, 24)), 24);
        assert_eq!(fill_count(&gauge(150.0, 24)), 24);
        assert_eq!(gauge(0.0, 10).chars().filter(|c| *c == '░').count(), 10);
    }

    #[test]
    fn human_bytes_cases() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
        assert_eq!(human_bytes(2 * 1024 * 1024 * 1024 * 1024), "2.0 TB");
    }

    #[test]
    fn human_duration_cases() {
        assert_eq!(human_duration(0), "0s");
        assert_eq!(human_ms(45_000), "45s");
        assert_eq!(human_duration(3), "3m");
        assert_eq!(human_duration(136), "2h16m");
        assert_eq!(human_duration(120), "2h");
        assert_eq!(human_duration(6 * 24 * 60), "6d");
        assert_eq!(human_duration(-136), "-2h16m");
    }
}

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(frame: usize) -> &'static str {
    SPINNER[frame % SPINNER.len()]
}
