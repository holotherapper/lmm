//! Terminal output formatting: colors, tables, progress, and clack-style prompts.
use std::fmt::Write;
use std::io::IsTerminal;
use std::sync::OnceLock;

const KB: u64 = 1024;
const MB: u64 = 1024 * KB;
const GB: u64 = 1024 * MB;
const TB: u64 = 1024 * GB;

pub fn bytes(value: u64) -> String {
    if value >= TB {
        format!("{:.1} TB", value as f64 / TB as f64)
    } else if value >= GB {
        format!("{:.1} GB", value as f64 / GB as f64)
    } else if value >= MB {
        format!("{:.1} MB", value as f64 / MB as f64)
    } else if value >= KB {
        format!("{:.1} KB", value as f64 / KB as f64)
    } else {
        format!("{value} B")
    }
}

pub fn count(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

// ── Color control ───────────────────────────────────────────────────────

static COLOR_ENABLED: OnceLock<bool> = OnceLock::new();

pub fn init_color(ui_color: &str) {
    let enabled = match ui_color {
        "always" => true,
        "never" => false,
        _ => {
            if std::env::var_os("NO_COLOR").is_some() {
                false
            } else {
                std::io::stderr().is_terminal()
            }
        }
    };
    let _ = COLOR_ENABLED.set(enabled);
}

pub fn use_color() -> bool {
    COLOR_ENABLED.get().copied().unwrap_or_else(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            false
        } else {
            std::io::stderr().is_terminal()
        }
    })
}

fn ansi(code: &str, text: &str) -> String {
    if use_color() {
        format!("{code}{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

pub fn dim(text: &str) -> String {
    ansi("\x1b[2m", text)
}

pub fn green(text: &str) -> String {
    ansi("\x1b[32m", text)
}

pub fn yellow(text: &str) -> String {
    ansi("\x1b[33m", text)
}

pub fn red(text: &str) -> String {
    ansi("\x1b[31m", text)
}

pub fn cyan(text: &str) -> String {
    ansi("\x1b[36m", text)
}

pub fn bold(text: &str) -> String {
    ansi("\x1b[1m", text)
}

pub fn error_badge() -> String {
    if use_color() {
        "\x1b[41;1;37m ERROR \x1b[0m".to_string()
    } else {
        "ERROR".to_string()
    }
}

// ── Banner ──────────────────────────────────────────────────────────────

const LOGO_LINES: &[&str] = &[
    r"______",
    r"___  /______ __________ ___",
    r"__  /__  __ `__ \_  __ `__ \",
    r"_  / _  / / / / /  / / / / /",
    r"/_/  /_/ /_/ /_//_/ /_/ /_/",
];

const GRAYS_256: &[&str] = &[
    "\x1b[38;5;250m",
    "\x1b[38;5;248m",
    "\x1b[38;5;245m",
    "\x1b[38;5;242m",
    "\x1b[38;5;238m",
];

pub fn logo() {
    eprintln!();
    if !use_color() {
        for line in LOGO_LINES {
            eprintln!("{line}");
        }
        return;
    }
    for (i, line) in LOGO_LINES.iter().enumerate() {
        let gray = GRAYS_256.get(i).unwrap_or(&"\x1b[38;5;240m");
        eprintln!("{gray}{line}\x1b[0m");
    }
}

pub fn banner() {
    logo();
    eprintln!();
    eprintln!("  {}", dim("Local AI model manager"));
    eprintln!();
    eprintln!(
        "  {} {}  {}",
        dim("$"),
        bold("lmm add <repo>"),
        dim("Install a model from Hugging Face")
    );
    eprintln!(
        "  {} {}      {}",
        dim("$"),
        bold("lmm remove"),
        dim("Remove installed models")
    );
    eprintln!(
        "  {} {}        {}",
        dim("$"),
        bold("lmm list"),
        dim("List all local models")
    );
    eprintln!(
        "  {} {} {}",
        dim("$"),
        bold("lmm search [query]"),
        dim("Search Hugging Face")
    );
    eprintln!();
    eprintln!(
        "  {} {}       {}",
        dim("$"),
        bold("lmm adopt"),
        dim("Adopt untracked HF Cache models")
    );
    eprintln!(
        "  {} {}      {}",
        dim("$"),
        bold("lmm update"),
        dim("Update tracked models")
    );
    eprintln!(
        "  {} {}      {}",
        dim("$"),
        bold("lmm doctor"),
        dim("Validate models and exposures")
    );
    eprintln!(
        "  {} {}          {}",
        dim("$"),
        bold("lmm gc"),
        dim("Clean stale files and blobs")
    );
    eprintln!();
    eprintln!("  try: {}", cyan("lmm search"));
    eprintln!();
}

// ── Clack-style flow (delegated to cliclack) ───────────────────────────

pub fn intro(text: &str) {
    let _ = cliclack::intro(text);
}

pub fn step(label: &str) {
    let _ = cliclack::log::step(label);
}

pub fn info(label: &str) {
    let _ = cliclack::log::info(label);
}

pub fn success(label: &str) {
    let _ = cliclack::log::success(label);
}

pub fn outro(text: &str) {
    let _ = cliclack::outro(text);
}

pub fn outro_cancel(text: &str) {
    let _ = cliclack::outro_cancel(text);
}

// ── Status line output ──────────────────────────────────────────────────

pub fn status(icon: &str, message: &str) {
    if use_color() {
        eprintln!("{icon} {message}");
    } else {
        eprintln!("{message}");
    }
}

pub fn heading(text: &str) {
    if use_color() {
        eprintln!("\x1b[1;36m{text}\x1b[0m");
    } else {
        eprintln!("{text}");
    }
}

pub fn kv(key: &str, value: &str) {
    if use_color() {
        eprintln!("  \x1b[2m{key:>20}\x1b[0m  {value}");
    } else {
        eprintln!("  {key:>20}  {value}");
    }
}

// ── Table ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Align {
    Left,
    Right,
}

pub struct Table {
    headers: Vec<String>,
    aligns: Vec<Align>,
    rows: Vec<Vec<String>>,
    widths: Vec<usize>,
}

impl Table {
    pub fn new(columns: &[(&str, Align)]) -> Self {
        let headers: Vec<String> = columns.iter().map(|(h, _)| (*h).to_string()).collect();
        let aligns: Vec<Align> = columns.iter().map(|(_, a)| *a).collect();
        let widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
        Self {
            headers,
            aligns,
            rows: Vec::new(),
            widths,
        }
    }

    pub fn row(&mut self, cells: &[&str]) {
        let row: Vec<String> = cells.iter().map(|c| strip_ansi(c)).collect();
        for (i, cell) in row.iter().enumerate() {
            if let Some(w) = self.widths.get_mut(i) {
                *w = (*w).max(cell.chars().count());
            }
        }
        let display_row: Vec<String> = cells.iter().map(|c| (*c).to_string()).collect();
        self.rows.push(display_row);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let col_count = self.headers.len();
        let color = use_color();

        let header_line = self.format_row_plain(&self.headers, col_count);
        if color {
            let _ = writeln!(out, "\x1b[1m{header_line}\x1b[0m");
        } else {
            let _ = writeln!(out, "{header_line}");
        }

        let sep: String = self
            .widths
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let s = "─".repeat(*w);
                if i + 1 < col_count {
                    format!("{s}  ")
                } else {
                    s
                }
            })
            .collect();
        if color {
            let _ = writeln!(out, "\x1b[2m{sep}\x1b[0m");
        } else {
            let _ = writeln!(out, "{sep}");
        }

        for row in &self.rows {
            let _ = writeln!(out, "{}", self.format_row(row, col_count));
        }
        out
    }

    fn format_row_plain(&self, cells: &[String], col_count: usize) -> String {
        let mut line = String::new();
        for i in 0..col_count {
            let cell = cells.get(i).map(String::as_str).unwrap_or("");
            let width = self.widths.get(i).copied().unwrap_or(0);
            let align = self.aligns.get(i).copied().unwrap_or(Align::Left);
            let pad = width.saturating_sub(cell.chars().count());
            match align {
                Align::Left => {
                    line.push_str(cell);
                    if i + 1 < col_count {
                        for _ in 0..pad {
                            line.push(' ');
                        }
                    }
                }
                Align::Right => {
                    for _ in 0..pad {
                        line.push(' ');
                    }
                    line.push_str(cell);
                }
            }
            if i + 1 < col_count {
                line.push_str("  ");
            }
        }
        line
    }

    fn format_row(&self, cells: &[String], col_count: usize) -> String {
        let mut line = String::new();
        for i in 0..col_count {
            let cell = cells.get(i).map(String::as_str).unwrap_or("");
            let width = self.widths.get(i).copied().unwrap_or(0);
            let align = self.aligns.get(i).copied().unwrap_or(Align::Left);
            let visible_len = strip_ansi(cell).chars().count();
            let pad = width.saturating_sub(visible_len);
            match align {
                Align::Left => {
                    line.push_str(cell);
                    if i + 1 < col_count {
                        for _ in 0..pad {
                            line.push(' ');
                        }
                    }
                }
                Align::Right => {
                    for _ in 0..pad {
                        line.push(' ');
                    }
                    line.push_str(cell);
                }
            }
            if i + 1 < col_count {
                line.push_str("  ");
            }
        }
        line
    }
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_escape = false;
    for ch in s.chars() {
        if in_escape {
            if ch.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else if ch == '\x1b' {
            in_escape = true;
        } else {
            out.push(ch);
        }
    }
    out
}

// ── Progress ────────────────────────────────────────────────────────────

pub struct ProgressLine {
    label: String,
    total: usize,
    current: usize,
}

impl ProgressLine {
    pub fn new(label: &str, total: usize) -> Self {
        Self {
            label: label.to_string(),
            total,
            current: 0,
        }
    }

    pub fn inc(&mut self, detail: &str) {
        self.current += 1;
        if use_color() {
            eprint!(
                "\r\x1b[2K\x1b[36m⟩\x1b[0m {} [{}/{}] {}",
                self.label, self.current, self.total, detail
            );
        } else {
            eprint!(
                "\r{} [{}/{}] {}",
                self.label, self.current, self.total, detail
            );
        }
    }

    pub fn finish(&self) {
        if use_color() {
            eprintln!(
                "\r\x1b[2K\x1b[32m✓\x1b[0m {} [{} done]",
                self.label, self.total
            );
        } else {
            eprintln!("{} [{} done]", self.label, self.total);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_human_readable() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1024), "1.0 KB");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(1_048_576), "1.0 MB");
        assert_eq!(bytes(1_073_741_824), "1.0 GB");
        assert_eq!(bytes(1_099_511_627_776), "1.0 TB");
    }

    #[test]
    fn format_count_human_readable() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1_500), "1.5K");
        assert_eq!(count(2_500_000), "2.5M");
    }

    #[test]
    fn table_renders_aligned_columns() {
        let mut table = Table::new(&[
            ("NAME", Align::Left),
            ("SIZE", Align::Right),
            ("STATUS", Align::Left),
        ]);
        table.row(&["model-a", "1.2 GB", "ok"]);
        table.row(&["model-long-name-b", "456 MB", "stale"]);
        let output = table.render();
        assert!(output.contains("model-a"));
        assert!(output.contains("model-long-name-b"));
    }

    #[test]
    fn strip_ansi_removes_escape_sequences() {
        assert_eq!(strip_ansi("\x1b[32mhello\x1b[0m"), "hello");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn no_color_functions_return_plain_text() {
        let _ = COLOR_ENABLED.set(false);
        assert_eq!(dim("test"), "test");
        assert_eq!(green("test"), "test");
        assert_eq!(bold("test"), "test");
    }
}
