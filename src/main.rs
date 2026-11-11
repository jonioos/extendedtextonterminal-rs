use etot_rs::{
    config::Config,
    lsp::{Client, Diagnostic, TextEdit},
    syntax::{self, Span, Style},
    terminal::TerminalPane,
    tooling::{self, ToolOutput},
    vim_regex,
    workspace::{Rect, SplitAxis, Workspace},
    Key, Mode,
};
use regex::Regex;
use std::borrow::Cow;
use std::collections::HashMap;
use std::env;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthChar;

fn cell_width(ch: char, used: usize) -> usize {
    if ch == '\t' {
        4 - used % 4
    } else {
        UnicodeWidthChar::width(ch).unwrap_or(0)
    }
}
fn visual_width(chars: impl Iterator<Item = char>) -> usize {
    let mut used = 0;
    for ch in chars {
        used += cell_width(ch, used);
    }
    used
}
fn canvas_background(config: &Config) -> &'static str {
    if config.ui.theme == "light" {
        "\x1b[48;2;241;243;249m"
    } else {
        "\x1b[48;2;34;36;53m"
    }
}
fn canvas_foreground(config: &Config) -> &'static str {
    if config.ui.theme == "light" {
        "\x1b[38;2;52;59;88m"
    } else {
        "\x1b[38;2;192;202;245m"
    }
}
fn file_icon(name: &str) -> &'static str {
    match std::path::Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
    {
        Some("rs") => "",
        Some("ts" | "tsx") => "",
        Some("js" | "jsx") => "",
        Some("py") => "",
        Some("go") => "",
        Some("c" | "h") => "",
        Some("cc" | "cpp" | "cxx" | "hpp" | "hxx") => "",
        Some("cs" | "vb") => "",
        Some("md" | "markdown") => "",
        Some("toml" | "json" | "yaml" | "yml") => "",
        Some("html" | "htm") => "",
        Some("css") => "",
        Some("sh" | "bash") => "",
        _ => "",
    }
}
fn active_line_background(config: &Config) -> &'static str {
    if config.ui.theme == "light" {
        "\x1b[48;2;226;230;241m"
    } else {
        "\x1b[48;2;42;45;66m"
    }
}
fn keep_cursor_visible(left: &mut usize, cursor: usize, available: usize) {
    if available == 0 {
        *left = 0;
    } else if cursor < *left {
        *left = cursor;
    } else if cursor >= *left + available {
        *left = cursor + 1 - available;
    }
}
fn visible_char(ch: char, start: usize, left: usize, available: usize) -> Option<String> {
    let cells = cell_width(ch, start);
    if cells == 0 {
        return (start > left && start <= left.saturating_add(available)).then(|| ch.to_string());
    }
    let visible_start = start.max(left);
    let visible_end = (start + cells).min(left.saturating_add(available));
    if visible_start >= visible_end {
        return None;
    }
    let visible_cells = visible_end - visible_start;
    if ch == '\t' || ch.is_control() || visible_cells != cells {
        Some(" ".repeat(visible_cells))
    } else {
        Some(ch.to_string())
    }
}

struct SyntaxCacheEntry {
    path: Option<PathBuf>,
    revision: u64,
    spans: Vec<Vec<Span>>,
    pending_since: Option<Instant>,
}
#[derive(Default)]
struct ViewportState {
    top: usize,
    left: usize,
    pane_tops: HashMap<usize, usize>,
    pane_lefts: HashMap<usize, usize>,
}
struct PaneLineContext<'a> {
    cursor_row: usize,
    width: usize,
    left: usize,
    config: &'a Config,
    diagnostics: &'a [Diagnostic],
    syntax_spans: &'a [Vec<Span>],
    search: Option<&'a Regex>,
    active_pane: bool,
}

struct Terminal {
    previous: String,
}
impl Terminal {
    fn new() -> io::Result<Self> {
        let output = Command::new("stty")
            .arg("-g")
            .stdin(Stdio::inherit())
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other("ETOT requires a terminal"));
        }
        let previous = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let status = Command::new("stty")
            .args(["raw", "-echo", "min", "0", "time", "1"])
            .status()?;
        if !status.success() {
            return Err(io::Error::other("cannot enable raw terminal mode"));
        }
        print!("\x1b[?1049h\x1b[?25l");
        io::stdout().flush()?;
        Ok(Self { previous })
    }
    fn suspend(&self) -> io::Result<()> {
        print!("\x1b[0 q\x1b[?25h\x1b[?1049l\x1b[0m");
        io::stdout().flush()?;
        if Command::new("stty").arg(&self.previous).status()?.success() {
            Ok(())
        } else {
            Err(io::Error::other("cannot restore terminal for shell"))
        }
    }
    fn resume(&self) -> io::Result<()> {
        if !Command::new("stty")
            .args(["raw", "-echo", "min", "0", "time", "1"])
            .status()?
            .success()
        {
            return Err(io::Error::other("cannot resume raw terminal mode"));
        }
        print!("\x1b[?1049h\x1b[?25l");
        io::stdout().flush()
    }
    fn run_shell(&self, command: Option<&str>) -> io::Result<std::process::ExitStatus> {
        self.suspend()?;
        let shell = env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
        let mut child = Command::new(shell);
        if let Some(text) = command {
            child.arg("-c").arg(text);
        }
        let result = child
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
        if command.is_some() {
            print!("\n[Press Enter to return to ETOT]");
            io::stdout().flush()?;
            let mut line = String::new();
            let _ = io::stdin().read_line(&mut line);
        }
        self.resume()?;
        result
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        print!("\x1b[0 q\x1b[?25h\x1b[?1049l\x1b[0m");
        let _ = io::stdout().flush();
        let _ = Command::new("stty").arg(&self.previous).status();
    }
}
fn cursor_shape(mode: Mode) -> &'static str {
    if mode == Mode::Insert {
        "\x1b[6 q"
    } else {
        "\x1b[2 q"
    }
}
fn size() -> (usize, usize) {
    let mut window = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let result = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut window) };
    if result != 0 || window.ws_row == 0 || window.ws_col == 0 {
        return (24, 80);
    }
    (
        usize::from(window.ws_row).max(3),
        usize::from(window.ws_col).max(20),
    )
}
fn clip_cells(text: &str, width: usize) -> String {
    let mut result = String::new();
    let mut used = 0;
    for ch in text.chars().filter(|ch| !ch.is_control()) {
        let cells = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cells > width {
            break;
        }
        result.push(ch);
        used += cells;
    }
    result
}
fn header_segment(out: &mut String, remaining: &mut usize, style: &str, text: &str) {
    if *remaining == 0 {
        return;
    }
    let segment = clip_cells(text, *remaining);
    *remaining = remaining.saturating_sub(visual_width(segment.chars()));
    out.push_str(style);
    out.push_str(&segment);
}
fn append_header(out: &mut String, workspace: &Workspace, config: &Config, width: usize) {
    let header_bg = if config.ui.theme == "light" {
        "\x1b[48;2;221;225;238m"
    } else {
        "\x1b[48;2;26;28;42m"
    };
    out.push_str(header_bg);
    out.push_str("\x1b[2K");
    let mut remaining = width.saturating_sub(1);
    header_segment(out, &mut remaining, "\x1b[38;2;122;162;247m", " * ETOT ");
    for (index, (name, active)) in workspace.tab_titles().into_iter().enumerate() {
        let style = if active {
            "\x1b[38;2;34;36;53;48;2;122;162;247m"
        } else {
            "\x1b[38;2;150;156;187m"
        };
        header_segment(
            out,
            &mut remaining,
            style,
            &format!(
                "  {} {}:{}  ",
                if active { ">" } else { "." },
                index + 1,
                name.chars().take(16).collect::<String>()
            ),
        );
        out.push_str(header_bg);
    }
    header_segment(out, &mut remaining, "\x1b[38;2;95;105;140m", " | ");
    for (name, modified, active) in workspace.tabs() {
        let style = if active {
            "\x1b[38;2;192;202;245m"
        } else {
            "\x1b[38;2;95;105;140m"
        };
        header_segment(
            out,
            &mut remaining,
            style,
            &format!(
                " {} {}{} ",
                file_icon(&name),
                name.chars().take(16).collect::<String>(),
                if modified { "+" } else { "" }
            ),
        );
        out.push_str(header_bg);
    }
    out.push_str("\x1b[0m");
}
fn append_statusline(
    out: &mut String,
    workspace: &Workspace,
    config: &Config,
    diagnostics: &[Diagnostic],
    width: usize,
    height: usize,
) {
    let editor = workspace.active();
    let (mode, badge, accent): (&str, &str, &str) = match editor.mode {
        Mode::Normal => ("NORMAL", "\x1b[48;2;122;162;247m", "\x1b[38;2;122;162;247m"),
        Mode::Insert => ("INSERT", "\x1b[48;2;158;206;106m", "\x1b[38;2;158;206;106m"),
        Mode::Visual | Mode::VisualLine | Mode::VisualBlock => {
            ("VISUAL", "\x1b[48;2;187;154;247m", "\x1b[38;2;187;154;247m")
        }
        Mode::Command => (
            "COMMAND",
            "\x1b[48;2;224;175;104m",
            "\x1b[38;2;224;175;104m",
        ),
        Mode::Replace | Mode::ReplaceOne => (
            "REPLACE",
            "\x1b[48;2;247;118;142m",
            "\x1b[38;2;247;118;142m",
        ),
        Mode::Search(_) => ("SEARCH", "\x1b[48;2;224;175;104m", "\x1b[38;2;224;175;104m"),
        Mode::OperatorPending => (
            "OPERATOR",
            "\x1b[48;2;125;207;255m",
            "\x1b[38;2;125;207;255m",
        ),
        _ => ("ETOT", "\x1b[48;2;125;207;255m", "\x1b[38;2;125;207;255m"),
    };
    let bar = if config.ui.theme == "light" {
        "\x1b[48;2;221;225;238;38;2;52;59;88m"
    } else {
        "\x1b[48;2;24;27;42;38;2;192;202;245m"
    };
    out.push_str(&format!(
        "\x1b[{height};1H{bar}\x1b[2K{badge}\x1b[38;2;24;27;42m {mode} {bar}{accent}{bar}"
    ));
    let name = editor
        .path
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "[No Name]".into());
    let file = format!(
        "  {} {}{}{}",
        file_icon(&name),
        name,
        if editor.modified { " *" } else { "" },
        if editor.readonly { " RO" } else { "" }
    );
    let errors = diagnostics.iter().filter(|diag| diag.severity == 1).count();
    let warnings = diagnostics.iter().filter(|diag| diag.severity == 2).count();
    let percent = (editor.row + 1) * 100 / editor.lines.len().max(1);
    let language = editor
        .path
        .as_ref()
        .and_then(|path| path.extension())
        .map(|ext| {
            ext.to_string_lossy()
                .chars()
                .filter(|ch| ch.is_alphanumeric())
                .take(8)
                .collect::<String>()
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "text".into());
    let right = format!(
        "  {} {language}  E:{errors}  W:{warnings}   {}%   {}:{} ",
        if workspace.window_count() > 1 {
            "||"
        } else {
            "-"
        },
        percent,
        editor.row + 1,
        editor.col + 1
    );
    let saved = editor
        .last_saved_at
        .and_then(etot_rs::datetime::hhmm)
        .unwrap_or_else(|| "--:--".into());
    let clock = format!("  {saved} ");
    let right_width = visual_width(right.chars()) + visual_width(clock.chars()) + 1;
    let show_right = width > right_width + mode.len() + 6;
    let file_width =
        width.saturating_sub(mode.len() + if show_right { right_width + 4 } else { 4 });
    out.push_str(&fit_cells(
        &file,
        file_width.min(visual_width(file.chars())),
    ));
    if show_right {
        let x = width - right_width + 1;
        out.push_str(&format!(
            "\x1b[{height};{x}H{bar}\x1b[38;2;122;162;247m{right}\x1b[48;2;122;162;247m\x1b[38;2;24;27;42m{clock}"
        ));
    }
    out.push_str("\x1b[0m");
}
struct HintView {
    title: String,
    choices: Vec<(String, String)>,
}
const EX_COMMANDS: &[(&str, &str)] = &[
    ("$end", "salva ed esci"),
    ("$quit", "esci con conferma"),
    ("$undo", "annulla modifica"),
    ("$redo", "ripristina modifica"),
    ("$copy", "copia selezione Visual"),
    ("w", "salva"),
    ("wa", "salva tutti"),
    ("wq", "salva ed esci"),
    ("q", "chiudi"),
    ("q!", "forza uscita"),
    ("e", "apri file"),
    ("read", "inserisci file"),
    ("bn", "buffer dopo"),
    ("bp", "buffer prima"),
    ("buffers", "lista buffer"),
    ("bd", "chiudi buffer"),
    ("split", "dividi orizz."),
    ("vsplit", "dividi vert."),
    ("tabnew", "nuova scheda"),
    ("tabnext", "scheda dopo"),
    ("tabprevious", "scheda prima"),
    ("tabclose", "chiudi scheda"),
    ("grep", "cerca progetto"),
    ("s", "sostituisci"),
    ("g", "filtra righe"),
    ("v", "filtra righe inverse"),
    ("sort", "ordina righe"),
    ("delete", "elimina righe"),
    ("yank", "copia righe"),
    ("put", "incolla righe"),
    ("join", "unisci righe"),
    ("copy", "duplica righe"),
    ("move", "sposta righe"),
    ("undo", "annulla"),
    ("redo", "ripeti"),
    ("set", "opzioni"),
    ("GitStatus", "stato Git"),
    ("GitDiff", "diff Git"),
    ("LspDiagnostics", "problemi"),
    ("LspSymbols", "simboli"),
    ("LspFormat", "formatta"),
    ("LspCodeActions", "correzioni"),
    ("LspRename", "rinomina"),
    ("Format", "formatta file"),
    ("Lint", "analizza file"),
    ("SessionSave", "salva sessione"),
    ("SessionLoad", "ripristina sessione"),
    ("terminal", "shell"),
    ("noh", "nascondi ricerca"),
];
const SET_OPTIONS: &[(&str, &str)] = &[
    ("number", "numeri di riga"),
    ("nonumber", "nascondi numeri"),
    ("relativenumber", "numeri relativi"),
    ("norelativenumber", "disattiva relativi"),
    ("background=dark", "tema scuro"),
    ("background=light", "tema chiaro"),
    ("ignorecase", "ricerca senza maiuscole"),
    ("noignorecase", "ricerca con maiuscole"),
    ("smartcase", "rispetta maiuscole nel pattern"),
    ("nosmartcase", "disattiva smartcase"),
    ("autoindent", "indentazione automatica"),
    ("noautoindent", "disattiva autoindent"),
    ("shiftwidth=4", "ampiezza indentazione"),
];
fn ex_matches(prefix: &str) -> Vec<&'static str> {
    let prefix = prefix.to_ascii_lowercase();
    EX_COMMANDS
        .iter()
        .filter(|(name, _)| name.to_ascii_lowercase().starts_with(&prefix))
        .map(|(name, _)| *name)
        .collect()
}
fn completion_matches(prefix: &str) -> Vec<String> {
    if let Some(option) = prefix.strip_prefix("set ") {
        let option = option.to_ascii_lowercase();
        return SET_OPTIONS
            .iter()
            .filter(|(name, _)| name.starts_with(&option))
            .map(|(name, _)| format!("set {name}"))
            .collect();
    }
    ex_matches(prefix).into_iter().map(str::to_owned).collect()
}
fn hint_view(workspace: &Workspace) -> Option<HintView> {
    let editor = workspace.active();
    if editor.mode == Mode::Command {
        let prefix = editor.prompt.trim_start();
        if let Some(option) = prefix.strip_prefix("set ") {
            let option = option.to_ascii_lowercase();
            let choices = SET_OPTIONS
                .iter()
                .filter(|(name, _)| name.starts_with(&option))
                .map(|(key, description)| ((*key).into(), (*description).into()))
                .collect::<Vec<_>>();
            return (!choices.is_empty()).then_some(HintView {
                title: "Opzioni".into(),
                choices,
            });
        }
        if prefix.contains(char::is_whitespace) {
            return None;
        }
        let choices = EX_COMMANDS
            .iter()
            .filter(|(name, _)| {
                name.to_ascii_lowercase()
                    .starts_with(&prefix.to_ascii_lowercase())
            })
            .take(6)
            .map(|(key, description)| ((*key).into(), (*description).into()))
            .collect::<Vec<_>>();
        return (!choices.is_empty()).then_some(HintView {
            title: "Comandi".into(),
            choices,
        });
    }
    workspace.command_hint().map(|hint| HintView {
        title: hint.title.into(),
        choices: hint
            .choices
            .iter()
            .map(|(key, description)| ((*key).into(), (*description).into()))
            .collect(),
    })
}
fn append_hint_overlay(
    out: &mut String,
    workspace: &Workspace,
    config: &Config,
    width: usize,
    height: usize,
) {
    let Some(hint) = hint_view(workspace) else {
        return;
    };
    let command_mode = (workspace.active().mode == Mode::Command
        || workspace.dollar_prefix_pending())
        && width >= 36
        && height >= 10;
    let box_width = if command_mode {
        width.saturating_sub(8).min(64)
    } else {
        width.saturating_sub(4).min(68)
    };
    let rows = hint.choices.len().div_ceil(2).min(3).min(if command_mode {
        height.saturating_sub(10)
    } else {
        3
    });
    if box_width < 28 || rows == 0 || height < rows + 8 {
        return;
    }
    let x = if command_mode {
        (width - box_width) / 2 + 1
    } else {
        3
    };
    let y = if command_mode { 7 } else { height - rows - 4 };
    let inside = box_width - 2;
    let title = if command_mode {
        format!(" {} ", hint.title)
    } else {
        format!(" ETOT  {} ", hint.title)
    };
    let title_len = title.chars().count().min(inside);
    let background = if config.ui.theme == "light" {
        "\x1b[48;2;235;238;247m"
    } else {
        "\x1b[48;2;26;28;42m"
    };
    let border = "\x1b[38;2;125;207;255m";
    out.push_str(&format!(
        "\x1b[{y};{x}H{background}{border}╭{}{}╮\x1b[0m",
        title.chars().take(title_len).collect::<String>(),
        "─".repeat(inside - title_len)
    ));
    let column_width = inside / 2;
    for row in 0..rows {
        out.push_str(&format!("\x1b[{};{x}H{background}{border}│", y + row + 1));
        for column in 0..2 {
            let index = row * 2 + column;
            let cell_width = if column == 0 {
                column_width
            } else {
                inside - column_width
            };
            let cell = hint
                .choices
                .get(index)
                .map(|(key, description)| format!(" {key}  {description}"))
                .unwrap_or_default();
            out.push_str(canvas_foreground(config));
            out.push_str(&fit_cells(&cell, cell_width));
            out.push_str(border);
        }
        out.push_str("│\x1b[0m");
    }
    out.push_str(&format!(
        "\x1b[{};{x}H{background}{border}╰{}╯\x1b[0m",
        y + rows + 1,
        "─".repeat(inside)
    ));
}
fn fit_cells(text: &str, width: usize) -> String {
    let mut result = String::new();
    let mut used = 0;
    for ch in text.chars().filter(|ch| !ch.is_control()) {
        let cells = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cells > width {
            break;
        }
        result.push(ch);
        used += cells;
    }
    result.push_str(&" ".repeat(width.saturating_sub(used)));
    result
}
fn append_command_popup(
    out: &mut String,
    workspace: &Workspace,
    config: &Config,
    width: usize,
    height: usize,
) -> Option<(usize, usize)> {
    let editor = workspace.active();
    let pending_dollar = workspace.dollar_prefix_pending();
    if (editor.mode != Mode::Command && !pending_dollar) || width < 36 || height < 10 {
        return None;
    }
    let box_width = width.saturating_sub(8).min(64);
    let inside = box_width - 2;
    let x = (width - box_width) / 2 + 1;
    let y = 4;
    let background = if config.ui.theme == "light" {
        "\x1b[48;2;235;238;247m"
    } else {
        "\x1b[48;2;26;28;42m"
    };
    let border = "\x1b[38;2;125;207;255m";
    let title = "─ Cmdline ";
    out.push_str(&format!(
        "\x1b[{y};{x}H{background}{border}╭{title}{}╮\x1b[0m",
        "─".repeat(inside - title.chars().count())
    ));
    let prompt = if pending_dollar { "$" } else { &editor.prompt };
    let sigil = if prompt.starts_with('$') { "" } else { ":" };
    let prefix = format!(" > {sigil}");
    let available = inside.saturating_sub(visual_width(prefix.chars()));
    let mut tail = Vec::new();
    let mut used = 0;
    for ch in prompt.chars().rev().filter(|ch| !ch.is_control()) {
        let cells = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cells > available {
            break;
        }
        tail.push(ch);
        used += cells;
    }
    tail.reverse();
    let display: String = tail.into_iter().collect();
    let content = format!("{prefix}{display}");
    out.push_str(&format!("\x1b[{};{x}H{background}{border}│\x1b[38;2;125;207;255m > \x1b[38;2;192;202;245m{sigil}{}{background}{border}│\x1b[0m", y + 1, fit_cells(&display, available)));
    out.push_str(&format!(
        "\x1b[{};{x}H{background}{border}╰{}╯\x1b[0m",
        y + 2,
        "─".repeat(inside)
    ));
    (!pending_dollar).then_some((
        y + 1,
        (x + 1 + visual_width(content.chars())).min(x + box_width - 2),
    ))
}
fn append_completion_popup(
    out: &mut String,
    editor: &etot_rs::Editor,
    width: usize,
    height: usize,
    cursor: (usize, usize),
) {
    if editor.mode != Mode::Completion
        || editor.completion_items.is_empty()
        || width < 26
        || height < 10
    {
        return;
    }
    let rows = editor
        .completion_items
        .len()
        .min(6)
        .min(height.saturating_sub(7));
    let box_width = editor
        .completion_items
        .iter()
        .map(|item| visual_width(item.chars().filter(|ch| !ch.is_control())))
        .max()
        .unwrap_or(0)
        .saturating_add(4)
        .clamp(24, width.saturating_sub(2).min(52));
    let x = cursor.1.max(1).min(width - box_width + 1);
    let y = if cursor.0 + rows + 2 < height - 1 {
        cursor.0 + 1
    } else {
        cursor.0.saturating_sub(rows + 2).max(2)
    };
    let inside = box_width - 2;
    let start = editor
        .completion_index
        .saturating_sub(2)
        .min(editor.completion_items.len() - rows);
    let title = format!(
        " Suggerimenti {}/{} ",
        editor.completion_index + 1,
        editor.completion_items.len()
    );
    let title = title.chars().take(inside).collect::<String>();
    out.push_str(&format!(
        "\x1b[{y};{x}H\x1b[96;48;5;236m╭{title}{}╮\x1b[0m",
        "─".repeat(inside - title.len())
    ));
    for line in 0..rows {
        let index = start + line;
        let selected = index == editor.completion_index;
        let marker = if selected { ">" } else { " " };
        let item = fit_cells(&editor.completion_items[index], inside.saturating_sub(2));
        let style = if selected {
            "\x1b[97;48;5;25m"
        } else {
            "\x1b[97;48;5;236m"
        };
        out.push_str(&format!(
            "\x1b[{};{x}H\x1b[96;48;5;236m│{style}{marker} {item}\x1b[96;48;5;236m│\x1b[0m",
            y + line + 1
        ));
    }
    out.push_str(&format!(
        "\x1b[{};{x}H\x1b[96;48;5;236m╰{}╯\x1b[0m",
        y + rows + 1,
        "─".repeat(inside)
    ));
}
fn append_dialog_popup(
    out: &mut String,
    workspace: &Workspace,
    config: &Config,
    width: usize,
    height: usize,
) -> Option<(usize, usize)> {
    let (title, message) = workspace.dialog_view()?;
    if width < 28 || height < 8 {
        return None;
    }
    let box_width = width.saturating_sub(4).min(76);
    let inside = box_width - 2;
    let x = (width - box_width) / 2 + 1;
    let y = (height / 3).max(3);
    let background = if config.ui.theme == "light" {
        "\x1b[48;2;235;238;247m"
    } else {
        "\x1b[48;2;26;28;42m"
    };
    let border = "\x1b[38;2;125;207;255m";
    let caption = format!("─ {title} ");
    out.push_str(&format!(
        "\x1b[{y};{x}H{background}{border}╭{caption}{}╮",
        "─".repeat(inside.saturating_sub(visual_width(caption.chars())))
    ));
    out.push_str(&format!(
        "\x1b[{};{x}H{background}{border}│\x1b[38;2;192;202;245m{}{}{border}│",
        y + 1,
        fit_cells(&message, inside),
        background
    ));
    out.push_str(&format!(
        "\x1b[{};{x}H{background}{border}╰{}╯\x1b[0m",
        y + 2,
        "─".repeat(inside)
    ));
    Some((y + 1, x + 2))
}
#[derive(Default)]
struct Input {
    pending: Option<u8>,
}
impl Input {
    fn byte(&mut self) -> io::Result<Option<u8>> {
        if let Some(byte) = self.pending.take() {
            return Ok(Some(byte));
        }
        let mut buffer = [0u8; 1];
        match io::stdin().lock().read(&mut buffer) {
            Ok(count) => Ok((count == 1).then_some(buffer[0])),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(None),
            Err(e) => Err(e),
        }
    }
    fn next_byte(&mut self) -> io::Result<u8> {
        loop {
            if let Some(byte) = self.byte()? {
                return Ok(byte);
            }
        }
    }
    fn read_key(&mut self) -> io::Result<Option<Key>> {
        let Some(first) = self.byte()? else {
            return Ok(None);
        };
        Ok(Some(match first {
            b'\r' | b'\n' => Key::Enter,
            127 | 8 => Key::Backspace,
            16 => Key::Ctrl('p'),
            14 => Key::Ctrl('n'),
            18 => Key::Ctrl('r'),
            3 => Key::Ctrl('c'),
            26 => Key::Ctrl('z'),
            28 => Key::Ctrl('\\'),
            22 => Key::Ctrl('v'),
            23 => Key::Ctrl('w'),
            15 => Key::Ctrl('o'),
            9 => Key::Ctrl('i'),
            4 => Key::Ctrl('d'),
            21 => Key::Ctrl('u'),
            20 => Key::Ctrl('t'),
            6 => Key::Ctrl('f'),
            2 => Key::Ctrl('b'),
            0 => Key::Ctrl(' '),
            27 => match self.byte()? {
                None => Key::Esc,
                Some(b'[') => match self.byte()? {
                    Some(b'A') => Key::Up,
                    Some(b'B') => Key::Down,
                    Some(b'C') => Key::Right,
                    Some(b'D') => Key::Left,
                    Some(b'H') => Key::Home,
                    Some(b'F') => Key::End,
                    Some(b'1') => match self.byte()? {
                        Some(b'~') => Key::Home,
                        _ => Key::Esc,
                    },
                    Some(b'4') => match self.byte()? {
                        Some(b'~') => Key::End,
                        _ => Key::Esc,
                    },
                    Some(b'5') => match self.byte()? {
                        Some(b'~') => Key::PageUp,
                        _ => Key::Esc,
                    },
                    Some(b'6') => match self.byte()? {
                        Some(b'~') => Key::PageDown,
                        _ => Key::Esc,
                    },
                    Some(b'3') => match self.byte()? {
                        Some(b'~') => Key::Delete,
                        _ => Key::Esc,
                    },
                    _ => Key::Esc,
                },
                Some(b'O') => match self.byte()? {
                    Some(b'H') => Key::Home,
                    Some(b'F') => Key::End,
                    _ => Key::Esc,
                },
                Some(other) => {
                    self.pending = Some(other);
                    Key::Esc
                }
            },
            x if x >= 0xC2 => {
                let count = if x < 0xE0 {
                    2
                } else if x < 0xF0 {
                    3
                } else {
                    4
                };
                let mut bytes = vec![x];
                for _ in 1..count {
                    bytes.push(self.next_byte()?)
                }
                Key::Char(
                    String::from_utf8(bytes)
                        .ok()
                        .and_then(|s| s.chars().next())
                        .unwrap_or('�'),
                )
            }
            x if x >= 32 => Key::Char(x as char),
            _ => Key::Esc,
        }))
    }
}
fn terminal_panel_size(height: usize, width: usize) -> Option<(u16, u16)> {
    (height >= 12 && width >= 24).then(|| {
        (
            (height / 3).max(4).min(height - 8).min(u16::MAX as usize) as u16,
            width.saturating_sub(4).min(u16::MAX as usize) as u16,
        )
    })
}
fn append_terminal_panel(
    out: &mut String,
    terminal: &TerminalPane,
    height: usize,
    width: usize,
) -> (usize, usize) {
    let (rows, cols) = terminal_panel_size(height, width).unwrap();
    let top = height - usize::from(rows) - 2;
    out.push_str(&format!("\x1b[{top};1H\x1b[48;2;26;28;42m\x1b[38;2;125;207;255m╭ Terminale · Ctrl+\\ chiudi {}╮\x1b[0m", "─".repeat(width.saturating_sub(29))));
    for (index, line) in terminal.screen().rows_formatted(0, cols).enumerate() {
        let y = top + index + 1;
        out.push_str(&format!("\x1b[{y};1H\x1b[48;2;26;28;42m\x1b[38;2;125;207;255m│ \x1b[0m{line}\x1b[0m\x1b[{y};{}H\x1b[38;2;125;207;255m│\x1b[0m", width - 1));
    }
    let (row, col) = terminal.screen().cursor_position();
    (top + usize::from(row) + 1, usize::from(col) + 3)
}
fn draw(
    workspace: &Workspace,
    viewport: &mut ViewportState,
    pane_syntax: &mut HashMap<usize, SyntaxCacheEntry>,
    config: &Config,
    diagnostics: &[Diagnostic],
    syntax_spans: &[Vec<Span>],
    terminal: Option<&TerminalPane>,
) -> io::Result<()> {
    if workspace.window_count() > 1 {
        return draw_split(
            workspace,
            viewport,
            pane_syntax,
            config,
            diagnostics,
            terminal,
        );
    }
    let top = &mut viewport.top;
    let left = &mut viewport.left;
    let editor = workspace.active();
    let (height, width) = size();
    let body = height.saturating_sub(3).max(1);
    if editor.row < *top {
        *top = editor.row
    }
    if editor.row >= *top + body {
        *top = editor.row + 1 - body
    }
    let available = width.saturating_sub(if config.ui.line_numbers { 5 } else { 0 });
    let cursor_visual = editor
        .lines
        .get(editor.row)
        .map(|line| visual_width(line.chars().take(editor.col)))
        .unwrap_or(0);
    keep_cursor_visible(left, cursor_visual, available);
    let mut out = format!(
        "\x1b[?2026h\x1b[?25l{}\x1b[0m\x1b[H",
        canvas_background(config)
    );
    append_header(&mut out, workspace, config, width);
    out.push_str("\r\n");
    let dim = if config.ui.theme == "light" {
        "\x1b[34m"
    } else {
        "\x1b[90m"
    };
    let match_color = if config.ui.theme == "light" {
        "\x1b[103;30m"
    } else {
        "\x1b[41m"
    };
    let highlight = editor
        .search
        .as_ref()
        .filter(|_| editor.highlight)
        .and_then(|(pattern, _)| vim_regex(pattern, editor.search_case_insensitive(pattern)).ok());
    for screen_row in 0..body {
        let row_background = if *top + screen_row == editor.row {
            active_line_background(config)
        } else {
            canvas_background(config)
        };
        out.push_str(row_background);
        out.push_str("\x1b[2K");
        out.push_str(canvas_foreground(config));
        let picker_line;
        let line = if editor.mode == Mode::Picker {
            picker_line = editor.picker_results.get(*top + screen_row).map(|path| {
                let relative = path
                    .strip_prefix(
                        editor
                            .project_root_path()
                            .unwrap_or_else(|| std::path::Path::new(".")),
                    )
                    .unwrap_or(path);
                format!(
                    "{}  {}",
                    file_icon(&path.to_string_lossy()),
                    relative.display()
                )
            });
            picker_line.as_ref()
        } else if editor.mode == Mode::CodeActions {
            picker_line = editor
                .code_actions
                .get(*top + screen_row)
                .map(|action| action.title.clone());
            picker_line.as_ref()
        } else if editor.mode == Mode::GitStatus {
            picker_line = editor
                .git_entries
                .get(*top + screen_row)
                .map(|entry| format!("{} {}", entry.status, entry.path.display()));
            picker_line.as_ref()
        } else if editor.mode == Mode::GitDiff {
            picker_line = editor.git_diff_lines.get(*top + screen_row).cloned();
            picker_line.as_ref()
        } else if editor.mode == Mode::Quickfix {
            picker_line = editor
                .quickfix
                .get(*top + screen_row)
                .map(|(path, row, _, text)| format!("{}:{}: {}", path.display(), row + 1, text));
            picker_line.as_ref()
        } else {
            editor.lines.get(*top + screen_row)
        };
        if let Some(line) = line {
            if config.ui.line_numbers {
                let diagnostic = diagnostics.iter().find(|d| d.row == *top + screen_row);
                let absolute = *top + screen_row + 1;
                let number = if config.ui.relative_numbers && absolute != editor.row + 1 {
                    absolute.abs_diff(editor.row + 1)
                } else {
                    absolute
                };
                out.push_str(if *top + screen_row == editor.row {
                    "\x1b[38;2;224;175;104m"
                } else {
                    dim
                });
                out.push_str(&format!(
                    "{number:>3}{} ",
                    if diagnostic.is_some() { "!" } else { " " }
                ));
                out.push_str(row_background);
                out.push_str(canvas_foreground(config));
            }
            let available = width.saturating_sub(if config.ui.line_numbers { 5 } else { 0 });
            let spans: Vec<_> = highlight
                .as_ref()
                .map(|re| re.find_iter(line).map(|m| (m.start(), m.end())).collect())
                .unwrap_or_default();
            let syntax = syntax_spans
                .get(*top + screen_row)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let mut match_index = 0;
            let mut syntax_index = 0;
            let mut used = 0;
            let mut active_style = "";
            for (column, (byte, ch)) in line.char_indices().enumerate() {
                let cells = cell_width(ch, used);
                if used >= left.saturating_add(available) {
                    break;
                }
                let segment = visible_char(ch, used, *left, available);
                used += cells;
                let Some(segment) = segment else { continue };
                while spans.get(match_index).is_some_and(|&(_, end)| end <= byte) {
                    match_index += 1;
                }
                let found = spans
                    .get(match_index)
                    .is_some_and(|&(start, end)| byte >= start && byte < end);
                while syntax
                    .get(syntax_index)
                    .is_some_and(|span| span.end <= byte)
                {
                    syntax_index += 1;
                }
                let active = editor.selection_contains(*top + screen_row, column) || found;
                let style = if active {
                    match_color
                } else {
                    syntax
                        .get(syntax_index)
                        .filter(|span| byte >= span.start && byte < span.end)
                        .map(|span| match span.style {
                            Style::Keyword => "\x1b[35m",
                            Style::String => "\x1b[32m",
                            Style::Comment => "\x1b[90m",
                            Style::Number => "\x1b[33m",
                            Style::Function => "\x1b[36m",
                            Style::Type => "\x1b[34m",
                        })
                        .unwrap_or(canvas_foreground(config))
                };
                if style != active_style {
                    if !active {
                        out.push_str(row_background);
                    }
                    out.push_str(style);
                    active_style = style
                }
                out.push_str(&segment);
            }
            if !active_style.is_empty() {
                out.push_str("\x1b[0m")
            }
        } else {
            out.push_str("\x1b[90m~\x1b[0m")
        }
        out.push_str("\r\n");
    }
    append_statusline(&mut out, workspace, config, diagnostics, width, height);
    out.push_str(&format!(
        "\x1b[{};1H{}\x1b[2K{}",
        height - 1,
        canvas_background(config),
        canvas_foreground(config)
    ));
    match editor.mode {
        Mode::Search(forward) => {
            out.push(if forward { '/' } else { '?' });
            out.push_str(&editor.prompt)
        }
        Mode::Command if width >= 36 && height >= 10 => {
            out.push_str(" > Tab: completa  |  Invio: esegui  |  Esc: chiudi")
        }
        Mode::Command => {
            if !editor.prompt.starts_with('$') {
                out.push(':');
            }
            out.push_str(&editor.prompt)
        }
        Mode::Picker => {
            out.push_str("Find file: ");
            out.push_str(&editor.prompt)
        }
        _ => out.push_str(&editor.message),
    }
    let cursor_row = editor.row.saturating_sub(*top) + 2;
    let cursor_col = editor
        .lines
        .get(editor.row)
        .map(|line| {
            visual_width(line.chars().take(editor.col)).saturating_sub(*left)
                + if config.ui.line_numbers { 6 } else { 1 }
        })
        .unwrap_or(if config.ui.line_numbers { 6 } else { 1 });
    let (cursor_row, cursor_col) =
        if matches!(editor.mode, Mode::Search(_) | Mode::Command | Mode::Picker) {
            (
                height - 1,
                editor.prompt.chars().count()
                    + if editor.mode == Mode::Picker {
                        12
                    } else if editor.mode == Mode::Command && editor.prompt.starts_with('$') {
                        1
                    } else {
                        2
                    },
            )
        } else {
            (cursor_row, cursor_col.min(width))
        };
    append_hint_overlay(&mut out, workspace, config, width, height);
    let (cursor_row, cursor_col) = append_command_popup(&mut out, workspace, config, width, height)
        .unwrap_or((cursor_row, cursor_col));
    append_completion_popup(&mut out, editor, width, height, (cursor_row, cursor_col));
    let (cursor_row, cursor_col) = terminal
        .map(|terminal| append_terminal_panel(&mut out, terminal, height, width))
        .unwrap_or((cursor_row, cursor_col));
    let (cursor_row, cursor_col) = append_dialog_popup(&mut out, workspace, config, width, height)
        .unwrap_or((cursor_row, cursor_col));
    out.push_str(&format!(
        "\x1b[{cursor_row};{cursor_col}H{}\x1b[?25h\x1b[?2026l",
        cursor_shape(editor.mode)
    ));
    io::stdout().write_all(out.as_bytes())?;
    io::stdout().flush()
}
fn pane_display_line(editor: &etot_rs::Editor, row: usize) -> Option<Cow<'_, str>> {
    match editor.mode {
        Mode::Picker => editor.picker_results.get(row).map(|path| {
            let relative = path
                .strip_prefix(
                    editor
                        .project_root_path()
                        .unwrap_or_else(|| std::path::Path::new(".")),
                )
                .unwrap_or(path);
            Cow::Owned(format!(
                "{}  {}",
                file_icon(&path.to_string_lossy()),
                relative.display()
            ))
        }),
        Mode::CodeActions => editor
            .code_actions
            .get(row)
            .map(|action| Cow::Borrowed(action.title.as_str())),
        Mode::GitStatus => editor
            .git_entries
            .get(row)
            .map(|entry| Cow::Owned(format!("{} {}", entry.status, entry.path.display()))),
        Mode::GitDiff => editor
            .git_diff_lines
            .get(row)
            .map(|line| Cow::Borrowed(line.as_str())),
        Mode::Quickfix => editor.quickfix.get(row).map(|(path, line, _, text)| {
            Cow::Owned(format!("{}:{}: {}", path.display(), line + 1, text))
        }),
        _ => editor
            .lines
            .get(row)
            .map(|line| Cow::Borrowed(line.as_str())),
    }
}
fn pane_render_line(editor: &etot_rs::Editor, row: usize, context: &PaneLineContext<'_>) -> String {
    let mut out = String::new();
    let active_row = context.active_pane && row == context.cursor_row;
    let row_background = if active_row {
        active_line_background(context.config)
    } else {
        canvas_background(context.config)
    };
    out.push_str(row_background);
    if active_row && context.width > 0 {
        out.push_str(&" ".repeat(context.width));
        out.push_str(&format!("\x1b[{}D", context.width));
    }
    out.push_str(canvas_foreground(context.config));
    let Some(line) = pane_display_line(editor, row) else {
        out.push_str("\x1b[90m~\x1b[0m");
        return out;
    };
    let numbers = context.config.ui.line_numbers && context.width > 6;
    if numbers {
        let absolute = row + 1;
        let number = if context.config.ui.relative_numbers && row != context.cursor_row {
            absolute.abs_diff(context.cursor_row + 1)
        } else {
            absolute
        };
        let diagnostic = context.active_pane && context.diagnostics.iter().any(|d| d.row == row);
        out.push_str(if context.active_pane && row == context.cursor_row {
            "\x1b[38;2;224;175;104m"
        } else {
            "\x1b[90m"
        });
        out.push_str(&format!(
            "{number:>3}{} ",
            if diagnostic { "!" } else { " " }
        ));
        out.push_str("\x1b[0m");
        out.push_str(row_background);
        out.push_str(canvas_foreground(context.config));
    }
    let available = context.width.saturating_sub(if numbers { 5 } else { 0 });
    let matches: Vec<_> = context
        .search
        .map(|re| {
            re.find_iter(&line)
                .map(|found| (found.start(), found.end()))
                .collect()
        })
        .unwrap_or_default();
    let syntax = context
        .syntax_spans
        .get(row)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut match_index = 0;
    let mut syntax_index = 0;
    let mut used = 0;
    let mut style = "";
    for (column, (byte, ch)) in line.char_indices().enumerate() {
        let cells = cell_width(ch, used);
        if used >= context.left.saturating_add(available) {
            break;
        }
        let segment = visible_char(ch, used, context.left, available);
        used += cells;
        let Some(segment) = segment else { continue };
        let selected = context.active_pane && editor.selection_contains(row, column);
        while matches
            .get(match_index)
            .is_some_and(|&(_, end)| end <= byte)
        {
            match_index += 1;
        }
        let found = matches
            .get(match_index)
            .is_some_and(|&(start, end)| byte >= start && byte < end);
        while syntax
            .get(syntax_index)
            .is_some_and(|span| span.end <= byte)
        {
            syntax_index += 1;
        }
        let next_style = if selected || found {
            "\x1b[41m"
        } else {
            syntax
                .get(syntax_index)
                .filter(|span| byte >= span.start && byte < span.end)
                .map(|span| match span.style {
                    Style::Keyword => "\x1b[35m",
                    Style::String => "\x1b[32m",
                    Style::Comment => "\x1b[90m",
                    Style::Number => "\x1b[33m",
                    Style::Function => "\x1b[36m",
                    Style::Type => "\x1b[34m",
                })
                .unwrap_or(canvas_foreground(context.config))
        };
        if next_style != style {
            if !selected && !found {
                out.push_str(row_background);
            }
            out.push_str(next_style);
            style = next_style;
        }
        out.push_str(&segment);
    }
    if !style.is_empty() {
        out.push_str("\x1b[0m");
    }
    out
}
fn draw_split(
    workspace: &Workspace,
    viewport: &mut ViewportState,
    syntax_cache: &mut HashMap<usize, SyntaxCacheEntry>,
    config: &Config,
    diagnostics: &[Diagnostic],
    terminal: Option<&TerminalPane>,
) -> io::Result<()> {
    let (height, width) = size();
    let (panes, dividers) = workspace.panes(Rect {
        x: 0,
        y: 1,
        width,
        height: height.saturating_sub(3),
    });
    let editor = workspace.active();
    let mut out = format!(
        "\x1b[?2026h\x1b[?25l{}\x1b[0m\x1b[H",
        canvas_background(config)
    );
    append_header(&mut out, workspace, config, width);
    for row in 2..height.saturating_sub(1) {
        out.push_str(&format!(
            "\x1b[{row};1H{}\x1b[2K",
            canvas_background(config)
        ));
    }
    for divider in dividers {
        match divider.axis {
            SplitAxis::Vertical => {
                if divider.rect.width > 0 {
                    for y in divider.rect.y..divider.rect.y + divider.rect.height {
                        out.push_str(&format!(
                            "\x1b[{};{}H\x1b[90m│\x1b[0m",
                            y + 1,
                            divider.rect.x + 1
                        ));
                    }
                }
            }
            SplitAxis::Horizontal => {
                if divider.rect.height > 0 {
                    out.push_str(&format!(
                        "\x1b[{};{}H\x1b[90m{}\x1b[0m",
                        divider.rect.y + 1,
                        divider.rect.x + 1,
                        "─".repeat(divider.rect.width.saturating_sub(1))
                    ));
                }
            }
        }
    }
    let mut cursor = (height, 1);
    for pane in panes {
        if pane.rect.width == 0 || pane.rect.height == 0 {
            continue;
        }
        let shown = workspace.buffer(pane.buffer);
        let path = shown.path.clone();
        let stale = syntax_cache
            .get(&pane.buffer)
            .is_none_or(|entry| entry.path != path || entry.revision != shown.revision);
        if stale {
            let pending_since = path
                .as_ref()
                .filter(|path| syntax::supported(path))
                .filter(|_| {
                    shown.lines.iter().map(String::len).sum::<usize>() <= syntax::MAX_SOURCE_BYTES
                })
                .map(|_| Instant::now());
            syntax_cache.insert(
                pane.buffer,
                SyntaxCacheEntry {
                    path,
                    revision: shown.revision,
                    spans: Vec::new(),
                    pending_since,
                },
            );
        }
        if let Some(entry) = syntax_cache.get_mut(&pane.buffer) {
            if entry
                .pending_since
                .is_some_and(|at| at.elapsed() >= Duration::from_millis(160))
            {
                entry.spans = entry
                    .path
                    .as_ref()
                    .map(|path| syntax::highlight(path, &shown.content()))
                    .unwrap_or_default();
                entry.pending_since = None;
            }
        }
        let spans = &syntax_cache[&pane.buffer].spans;
        let name = shown
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "[No Name]".into());
        let title = format!(
            " {} {} {}{} ",
            if pane.active { '>' } else { ' ' },
            file_icon(&name),
            name,
            if shown.modified { "+" } else { "" }
        );
        out.push_str(&format!(
            "\x1b[{};{}H{}{}\x1b[0m",
            pane.rect.y + 1,
            pane.rect.x + 1,
            if pane.active {
                "\x1b[38;2;34;36;53;48;2;122;162;247m"
            } else {
                "\x1b[38;2;150;156;187;48;2;26;28;42m"
            },
            fit_cells(&title, pane.rect.width.saturating_sub(1))
        ));
        let body = pane.rect.height.saturating_sub(1);
        let top = viewport.pane_tops.entry(pane.window).or_default();
        if pane.cursor.0 < *top {
            *top = pane.cursor.0;
        }
        if body > 0 && pane.cursor.0 >= *top + body {
            *top = pane.cursor.0 + 1 - body;
        }
        let numbers = config.ui.line_numbers && pane.rect.width > 6;
        let pane_width = pane
            .rect
            .width
            .saturating_sub(usize::from(pane.rect.x + pane.rect.width >= width));
        let available = pane_width.saturating_sub(if numbers { 5 } else { 0 });
        let visual_col = shown
            .lines
            .get(pane.cursor.0)
            .map(|line| visual_width(line.chars().take(pane.cursor.1)))
            .unwrap_or(0);
        let left = viewport.pane_lefts.entry(pane.window).or_default();
        keep_cursor_visible(left, visual_col, available);
        let search = shown
            .search
            .as_ref()
            .filter(|_| shown.highlight)
            .and_then(|(pattern, _)| {
                vim_regex(pattern, shown.search_case_insensitive(pattern)).ok()
            });
        let line_context = PaneLineContext {
            cursor_row: pane.cursor.0,
            width: pane_width,
            left: *left,
            config,
            diagnostics: if pane.active { diagnostics } else { &[] },
            syntax_spans: spans,
            search: search.as_ref(),
            active_pane: pane.active,
        };
        for screen_row in 0..body {
            let file_row = *top + screen_row;
            let line = pane_render_line(shown, file_row, &line_context);
            out.push_str(&format!(
                "\x1b[{};{}H{}",
                pane.rect.y + screen_row + 2,
                pane.rect.x + 1,
                line
            ));
        }
        if pane.active && body > 0 {
            cursor = (
                pane.rect.y + 2 + pane.cursor.0.saturating_sub(*top),
                (pane.rect.x + 1 + if numbers { 5 } else { 0 } + visual_col.saturating_sub(*left))
                    .min(pane.rect.x + pane.rect.width)
                    .min(width),
            );
        }
    }
    append_statusline(&mut out, workspace, config, diagnostics, width, height);
    out.push_str(&format!(
        "\x1b[{};1H{}\x1b[2K{}",
        height - 1,
        canvas_background(config),
        canvas_foreground(config)
    ));
    match editor.mode {
        Mode::Search(forward) => {
            out.push(if forward { '/' } else { '?' });
            out.push_str(&editor.prompt);
        }
        Mode::Command if width >= 36 && height >= 10 => {
            out.push_str(" > Tab: completa  |  Invio: esegui  |  Esc: chiudi")
        }
        Mode::Command => {
            if !editor.prompt.starts_with('$') {
                out.push(':');
            }
            out.push_str(&editor.prompt);
        }
        Mode::Picker => {
            out.push_str("Find file: ");
            out.push_str(&editor.prompt);
        }
        _ => out.push_str(&editor.message),
    }
    if matches!(editor.mode, Mode::Search(_) | Mode::Command | Mode::Picker) {
        cursor = (
            height - 1,
            editor.prompt.chars().count()
                + if editor.mode == Mode::Picker {
                    12
                } else if editor.mode == Mode::Command && editor.prompt.starts_with('$') {
                    1
                } else {
                    2
                },
        );
    }
    append_hint_overlay(&mut out, workspace, config, width, height);
    cursor = append_command_popup(&mut out, workspace, config, width, height).unwrap_or(cursor);
    append_completion_popup(&mut out, editor, width, height, cursor);
    cursor = terminal
        .map(|terminal| append_terminal_panel(&mut out, terminal, height, width))
        .unwrap_or(cursor);
    cursor = append_dialog_popup(&mut out, workspace, config, width, height).unwrap_or(cursor);
    out.push_str(&format!(
        "\x1b[{};{}H{}\x1b[?25h\x1b[?2026l",
        cursor.0.min(height),
        cursor.1.min(width),
        cursor_shape(editor.mode)
    ));
    io::stdout().write_all(out.as_bytes())?;
    io::stdout().flush()
}
fn run() -> io::Result<()> {
    let mut path = None;
    let mut readonly = false;
    let mut edit = false;
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("Jonio Extended Text On Terminal 0.1.0");
                return Ok(());
            }
            "--help" | "-h" => {
                println!("etot [--edit|--read] [file|directory]\nNormal: i/I/a/A insert, Esc normal, ? or / search, Ctrl+P files\nNavigation: Home/End line, PageUp/PageDown screen; $ opens ETOT commands\nVim: . repeat, \"a register, \"+ clipboard, qa/@a macro, Ctrl+w windows, gt/gT tabs\nCommands: :w :wa :q :q! :wq $end [FILE] $quit $undo $redo; Visual: $copy\nMore: :e FILE :bn :bp :buffers :bd :split :vsplit :tabnew :tabnext :tabprevious :tabclose :grep PATTERN :Format :Lint :SessionSave :SessionLoad :GitStatus :GitDiff :!COMMAND :terminal\nInsert: type datetime or timeHMS, then Enter to insert local date or time\nTerminal panel: Ctrl+\\ closes\nLSP: gd definition, gr references, K hover, Ctrl+Space completion\nLSP commands: :LspDiagnostics :LspFormat :LspSymbols :LspCodeActions :LspRename NAME");
                return Ok(());
            }
            "--read" => readonly = true,
            "--edit" => edit = true,
            _ if path.is_none() && !arg.starts_with('-') => path = Some(PathBuf::from(arg)),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown argument: {arg}"),
                ))
            }
        }
    }
    if readonly && edit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--read and --edit cannot be combined",
        ));
    }
    let mut config = Config::load()?;
    let mut workspace = Workspace::open(path, readonly)?;
    if let Err(error) = workspace.restore_journals() {
        workspace.active_mut().message = format!("Recovery: {error}");
    }
    if edit && workspace.active().mode != Mode::Explorer {
        workspace.active_mut().begin_insert()
    }
    let terminate = Arc::new(AtomicBool::new(false));
    for signal in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGINT,
    ] {
        signal_hook::flag::register(signal, Arc::clone(&terminate))?;
    }
    let resized = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGWINCH, Arc::clone(&resized))?;
    let _terminal = Terminal::new()?;
    let mut viewport = ViewportState::default();
    let mut pane_syntax: HashMap<usize, SyntaxCacheEntry> = HashMap::new();
    let mut input = Input::default();
    let mut terminal_panel: Option<TerminalPane> = None;
    let mut ex_completion: Option<(String, usize, String)> = None;
    let mut lsp_path: Option<std::path::PathBuf> = None;
    let mut lsp_revision = 0;
    let mut lsp: Option<Client> = None;
    let mut lsp_pending: Option<Receiver<(PathBuf, u64, io::Result<Client>)>> = None;
    let mut tool_pending: Option<Receiver<(bool, PathBuf, u64, PathBuf, io::Result<ToolOutput>)>> =
        None;
    let mut lsp_seen_revision = workspace.active().revision;
    let mut lsp_changed_at: Option<Instant> = None;
    let mut syntax_path: Option<std::path::PathBuf> = None;
    let mut syntax_revision = u64::MAX;
    let mut syntax_spans: Vec<Vec<Span>> = Vec::new();
    let mut syntax_pending_since: Option<Instant> = None;
    let mut journal_seen = workspace.journal_signatures();
    let mut journal_changed_at: Option<Instant> = None;
    let mut redraw = true;
    while !workspace.quit {
        workspace.configure_options(
            config.search.ignorecase,
            config.search.smartcase,
            config.editing.autoindent,
            config.editing.shiftwidth,
        );
        if terminate.load(Ordering::Relaxed) {
            break;
        }
        if resized.swap(false, Ordering::Relaxed) {
            redraw = true;
        }
        let (height, width) = size();
        if terminal_panel.is_some() && terminal_panel_size(height, width).is_none() {
            terminal_panel = None;
            workspace.active_mut().message = "Terminal closed: screen too small".into();
            redraw = true;
        }
        if let Some(terminal) = terminal_panel.as_mut() {
            let (rows, cols) = terminal_panel_size(height, width).unwrap();
            terminal.resize(rows, cols)?;
            if terminal.poll() {
                redraw = true;
            }
            if terminal.exited()? {
                terminal_panel = None;
                workspace.active_mut().message = "Terminal exited".into();
                redraw = true;
            }
        }
        let journal_now = workspace.journal_signatures();
        if journal_now != journal_seen {
            journal_seen = journal_now;
            journal_changed_at = Some(Instant::now());
        }
        if journal_changed_at.is_some_and(|at| at.elapsed() >= Duration::from_millis(900)) {
            if let Err(error) = workspace.persist_journals() {
                workspace.active_mut().message = format!("Recovery: {error}");
                redraw = true;
            }
            journal_changed_at = None;
        }
        let current_path = workspace.active().path.clone();
        if current_path != lsp_path {
            lsp = None;
            lsp_pending = None;
            lsp_path = current_path.clone();
            lsp_seen_revision = workspace.active().revision;
            lsp_changed_at = None;
            if let Some(path) = current_path.as_ref().filter(|path| path.exists()) {
                let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
                if let Some(server) = config.server_for(extension) {
                    let root = workspace
                        .active()
                        .project_root_path()
                        .unwrap_or_else(|| {
                            path.parent().unwrap_or_else(|| std::path::Path::new("."))
                        })
                        .to_path_buf();
                    let path = path.clone();
                    let text = workspace.active().content();
                    let revision = workspace.active().revision;
                    let (sender, receiver) = mpsc::channel();
                    std::thread::spawn(move || {
                        let result = Client::start(
                            &server.command,
                            &root,
                            &path,
                            &server.language_id,
                            &text,
                        );
                        let _ = sender.send((path, revision, result));
                    });
                    lsp_pending = Some(receiver);
                }
            }
        }
        if let Some(result) = lsp_pending.as_ref().map(Receiver::try_recv) {
            match result {
                Ok((path, revision, client)) => {
                    lsp_pending = None;
                    if current_path.as_ref() == Some(&path) {
                        match client {
                            Ok(client) => {
                                lsp_revision = revision;
                                lsp = Some(client);
                                redraw = true;
                            }
                            Err(error) => {
                                workspace.active_mut().message =
                                    format!("LSP unavailable: {error}");
                                redraw = true;
                            }
                        }
                    }
                }
                Err(TryRecvError::Disconnected) => lsp_pending = None,
                Err(TryRecvError::Empty) => (),
            }
        }
        if let Some(result) = tool_pending.as_ref().map(Receiver::try_recv) {
            match result {
                Ok((formatting, path, revision, cwd, result)) => {
                    tool_pending = None;
                    if workspace.active().path.as_ref() == Some(&path)
                        && workspace.active().revision == revision
                    {
                        match result {
                            Ok(output) if formatting && output.success => {
                                let editor = workspace.active_mut();
                                let before = editor.content();
                                if before == output.stdout {
                                    editor.message = "Already formatted".into();
                                } else {
                                    let end = if before.ends_with('\n') {
                                        (editor.lines.len(), 0)
                                    } else {
                                        (
                                            editor.lines.len() - 1,
                                            editor.lines.last().unwrap().encode_utf16().count(),
                                        )
                                    };
                                    let edit = TextEdit {
                                        start: (0, 0),
                                        end,
                                        new_text: output.stdout,
                                    };
                                    editor.message = match editor.apply_lsp_edits(&[edit]) {
                                        Ok(_) => "Formatted buffer; save to write".into(),
                                        Err(error) => format!("Format: {error}"),
                                    };
                                }
                            }
                            Ok(output) if formatting => {
                                workspace.active_mut().message = format!(
                                    "Format: {}",
                                    output.stderr.lines().next().unwrap_or("formatter failed")
                                );
                            }
                            Ok(output) => {
                                let entries = tooling::diagnostics(&output, &path, &cwd);
                                if entries.is_empty() {
                                    workspace.active_mut().message = if output.success {
                                        "Lint clean".into()
                                    } else {
                                        format!(
                                            "Lint: {}",
                                            output
                                                .stderr
                                                .lines()
                                                .chain(output.stdout.lines())
                                                .next()
                                                .unwrap_or("linter failed")
                                        )
                                    };
                                } else {
                                    workspace.active_mut().show_quickfix(entries, "lint");
                                }
                            }
                            Err(error) => workspace.active_mut().message = format!("Tool: {error}"),
                        }
                        redraw = true;
                    }
                }
                Err(TryRecvError::Disconnected) => tool_pending = None,
                Err(TryRecvError::Empty) => (),
            }
        }
        if workspace.active().revision != lsp_seen_revision {
            lsp_seen_revision = workspace.active().revision;
            lsp_changed_at = Some(Instant::now());
        }
        if let Some(client) = lsp.as_mut() {
            if workspace.active().revision != lsp_revision
                && lsp_changed_at.is_some_and(|at| at.elapsed() >= Duration::from_millis(180))
            {
                let text = workspace.active().content();
                if let Err(e) = client.did_change(&text) {
                    workspace.active_mut().message = format!("LSP: {e}")
                }
                lsp_revision = workspace.active().revision;
            }
            if client.poll() {
                redraw = true
            }
        }
        if workspace.active().path != syntax_path || workspace.active().revision != syntax_revision
        {
            syntax_path = workspace.active().path.clone();
            syntax_revision = workspace.active().revision;
            syntax_spans.clear();
            syntax_pending_since = syntax_path
                .as_ref()
                .filter(|path| syntax::supported(path))
                .filter(|_| {
                    workspace
                        .active()
                        .lines
                        .iter()
                        .map(String::len)
                        .sum::<usize>()
                        <= syntax::MAX_SOURCE_BYTES
                })
                .map(|_| Instant::now());
        }
        if syntax_pending_since.is_some_and(|at| at.elapsed() >= Duration::from_millis(160)) {
            syntax_spans = syntax_path
                .as_ref()
                .map(|path| syntax::highlight(path, &workspace.active().content()))
                .unwrap_or_default();
            syntax_pending_since = None;
            redraw = true;
        }
        if !redraw
            && pane_syntax.values().any(|entry| {
                entry
                    .pending_since
                    .is_some_and(|at| at.elapsed() >= Duration::from_millis(160))
            })
        {
            redraw = true;
        }
        let diagnostics = lsp
            .as_ref()
            .map(|client| client.diagnostics.as_slice())
            .unwrap_or(&[]);
        if redraw {
            draw(
                &workspace,
                &mut viewport,
                &mut pane_syntax,
                &config,
                diagnostics,
                &syntax_spans,
                terminal_panel.as_ref(),
            )?;
            let (height, width) = size();
            let viewport = if workspace.window_count() > 1 {
                let (panes, _) = workspace.panes(Rect {
                    x: 0,
                    y: 1,
                    width,
                    height: height.saturating_sub(3),
                });
                panes
                    .into_iter()
                    .find(|pane| pane.active)
                    .map(|pane| {
                        (
                            *viewport.pane_tops.get(&pane.window).unwrap_or(&0),
                            pane.rect.height.saturating_sub(1).max(1),
                        )
                    })
                    .unwrap_or((0, 1))
            } else {
                (viewport.top, height.saturating_sub(3).max(1))
            };
            workspace.active_mut().set_viewport(viewport.0, viewport.1);
            redraw = false
        }
        let Some(key) = input.read_key()? else {
            continue;
        };
        redraw = true;
        if let Some(terminal) = terminal_panel.as_mut() {
            if key == Key::Ctrl('\\') {
                terminal_panel = None;
                workspace.active_mut().message = "Terminal closed".into();
            } else if let Err(error) = terminal.send(key) {
                workspace.active_mut().message = format!("Terminal: {error}");
                terminal_panel = None;
            }
            continue;
        }
        if workspace.has_dialog() {
            workspace.handle(key);
            continue;
        }
        if key == Key::Ctrl('c') {
            workspace.handle(Key::Esc);
            continue;
        }
        if matches!(key, Key::PageUp | Key::PageDown)
            && matches!(
                workspace.active().mode,
                Mode::Normal
                    | Mode::Insert
                    | Mode::Visual
                    | Mode::VisualLine
                    | Mode::VisualBlock
                    | Mode::Replace
            )
        {
            let body = size().0.saturating_sub(3).max(1);
            workspace
                .active_mut()
                .scroll_rows(key == Key::PageDown, body);
            continue;
        }
        if workspace.active().mode == Mode::Command && key == Key::Ctrl('i') {
            let current = workspace.active().prompt.clone();
            let (prefix, next) = match ex_completion.take() {
                Some((prefix, next, last)) if current == last => (prefix, next),
                _ => (current, 0),
            };
            let matches = completion_matches(&prefix);
            if !matches.is_empty() {
                let choice = matches[next % matches.len()].clone();
                workspace.active_mut().prompt = choice.clone();
                ex_completion = Some((prefix, next + 1, choice));
            }
            continue;
        }
        if key == Key::Enter && workspace.active().mode == Mode::Command {
            let prompt = workspace.active().prompt.trim().to_owned();
            if prompt == "Format" || prompt == "Lint" {
                let formatting = prompt == "Format";
                let editor = workspace.active_mut();
                editor.mode = Mode::Normal;
                editor.prompt.clear();
                if tool_pending.is_some() {
                    editor.message = "A formatter or linter is already running".into();
                    continue;
                }
                if formatting && editor.readonly {
                    editor.message = "Read-only buffer".into();
                    continue;
                }
                let Some(path) = editor.path.clone() else {
                    editor.message = "Open a named file first".into();
                    continue;
                };
                let extension = path
                    .extension()
                    .and_then(|part| part.to_str())
                    .unwrap_or("");
                let spec = if formatting {
                    config.formatter_for(extension)
                } else {
                    config.linter_for(extension)
                };
                let Some(spec) = spec else {
                    editor.message = format!(
                        "No {} configured for .{extension}",
                        if formatting { "formatter" } else { "linter" }
                    );
                    continue;
                };
                let cwd = editor
                    .project_root_path()
                    .or_else(|| path.parent())
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .to_path_buf();
                let revision = editor.revision;
                let content = editor.content();
                editor.message = if formatting {
                    "Formatting...".into()
                } else {
                    "Linting...".into()
                };
                let (sender, receiver) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = tooling::run(&spec, &path, &cwd, &content);
                    let _ = sender.send((formatting, path, revision, cwd, result));
                });
                tool_pending = Some(receiver);
                continue;
            }
            if prompt == "set" || prompt.starts_with("set ") {
                let option = prompt.strip_prefix("set").unwrap().trim();
                let result = config.apply_set(option);
                let editor = workspace.active_mut();
                editor.mode = Mode::Normal;
                editor.prompt.clear();
                editor.message = result.unwrap_or_else(|error| error);
                continue;
            }
            if let Some(new_name) = prompt.strip_prefix("LspRename ") {
                let name = new_name.trim();
                let (row, col_utf16) = {
                    let editor = workspace.active();
                    (
                        editor.row,
                        editor.lines[editor.row]
                            .chars()
                            .take(editor.col)
                            .map(char::len_utf16)
                            .sum(),
                    )
                };
                let result = if name.is_empty() {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "New name required",
                    ))
                } else if let Some(client) = lsp.as_mut() {
                    client.rename(row, col_utf16, name)
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "No language server for this file",
                    ))
                };
                workspace.active_mut().mode = Mode::Normal;
                workspace.active_mut().prompt.clear();
                match result {
                    Ok(changes) => {
                        let files = changes.len();
                        match workspace.apply_workspace_edits(changes) {
                            Ok(0) => workspace.active_mut().message = "No rename changes".into(),
                            Ok(edits) => workspace.active_mut().message = format!(
                                "Renamed {edits} occurrences in {files} buffers; save each modified buffer"),
                            Err(e) => workspace.active_mut().message = format!("LSP rename: {e}"),
                        }
                    }
                    Err(e) => workspace.active_mut().message = format!("LSP rename: {e}"),
                }
                continue;
            }
            if prompt == "terminal" {
                let cwd = workspace
                    .active()
                    .project_root_path()
                    .or_else(|| {
                        workspace.active().path.as_deref().and_then(|path| {
                            if path.is_dir() {
                                Some(path)
                            } else {
                                path.parent()
                            }
                        })
                    })
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .to_path_buf();
                let editor = workspace.active_mut();
                editor.mode = Mode::Normal;
                editor.prompt.clear();
                let (height, width) = size();
                editor.message = match terminal_panel_size(height, width) {
                    Some((rows, cols)) => match TerminalPane::start(rows, cols, &cwd) {
                        Ok(panel) => {
                            terminal_panel = Some(panel);
                            "Terminal opened".into()
                        }
                        Err(error) => format!("Terminal: {error}"),
                    },
                    None => "Terminal needs at least 24x12 cells".into(),
                };
                continue;
            }
            if prompt.starts_with('!') {
                let editor = workspace.active_mut();
                editor.mode = Mode::Normal;
                editor.prompt.clear();
                if prompt == "!" {
                    editor.message = "Shell command required".into();
                } else {
                    let command = prompt.strip_prefix('!');
                    editor.message = match _terminal.run_shell(command) {
                        Ok(status) => format!("Shell exited: {status}"),
                        Err(e) => format!("Shell: {e}"),
                    };
                }
                continue;
            }
        }
        if workspace.active().mode == Mode::Normal {
            if let Key::Char(c) = key {
                if let Some(mapping) = config.keys.normal.get(&c.to_string()) {
                    for mapped in mapping.chars() {
                        workspace.handle(Key::Char(mapped))
                    }
                    continue;
                }
            }
        }
        if key == Key::Enter
            && workspace.active().mode == Mode::Command
            && workspace.active().prompt.trim() == "LspDiagnostics"
        {
            if let Some(client) = lsp.as_ref() {
                let path = workspace.active().path.clone();
                if let Some(path) = path {
                    let entries = client
                        .diagnostics
                        .iter()
                        .map(|diag| (path.clone(), diag.row, diag.col, diag.message.clone()))
                        .collect();
                    let editor = workspace.active_mut();
                    editor.show_quickfix(entries, "LSP diagnostics");
                    editor.prompt.clear();
                }
            } else {
                workspace.active_mut().message = "No language server for this file".into();
                workspace.active_mut().mode = Mode::Normal;
                workspace.active_mut().prompt.clear()
            }
            continue;
        }
        if key == Key::Enter
            && workspace.active().mode == Mode::Command
            && workspace.active().prompt.trim() == "LspFormat"
        {
            let result = lsp.as_mut().map(Client::formatting);
            let editor = workspace.active_mut();
            editor.mode = Mode::Normal;
            editor.prompt.clear();
            editor.message = match result {
                Some(Ok(edits)) => match editor.apply_lsp_edits(&edits) {
                    Ok(0) => "Already formatted".into(),
                    Ok(count) => format!("Applied {count} formatting edits"),
                    Err(e) => format!("LSP format: {e}"),
                },
                Some(Err(e)) => format!("LSP format: {e}"),
                None => "No language server for this file".into(),
            };
            continue;
        }
        if key == Key::Enter
            && workspace.active().mode == Mode::Command
            && workspace.active().prompt.trim() == "LspSymbols"
        {
            let result = lsp.as_mut().map(Client::symbols);
            let path = workspace.active().path.clone();
            let editor = workspace.active_mut();
            editor.mode = Mode::Normal;
            editor.prompt.clear();
            match (result, path) {
                (Some(Ok(symbols)), Some(path)) => editor.show_quickfix(
                    symbols
                        .into_iter()
                        .map(|(name, row, col)| (path.clone(), row, col, name))
                        .collect(),
                    "symbols",
                ),
                (Some(Err(e)), _) => editor.message = format!("LSP symbols: {e}"),
                _ => editor.message = "No language server for this file".into(),
            }
            continue;
        }
        if key == Key::Enter
            && workspace.active().mode == Mode::Command
            && workspace.active().prompt.trim() == "LspCodeActions"
        {
            let editor = workspace.active();
            let col_utf16 = editor.lines[editor.row]
                .chars()
                .take(editor.col)
                .map(char::len_utf16)
                .sum();
            let result = lsp
                .as_mut()
                .map(|client| client.code_actions(editor.row, col_utf16));
            let editor = workspace.active_mut();
            editor.mode = Mode::Normal;
            editor.prompt.clear();
            match result {
                Some(Ok(actions)) => editor.show_code_actions(actions),
                Some(Err(e)) => editor.message = format!("LSP code actions: {e}"),
                None => editor.message = "No language server for this file".into(),
            }
            continue;
        }
        if key == Key::Ctrl(' ') && matches!(workspace.active().mode, Mode::Normal | Mode::Insert) {
            if let Some(client) = lsp.as_mut() {
                let editor = workspace.active();
                let col_utf16 = editor.lines[editor.row]
                    .chars()
                    .take(editor.col)
                    .map(char::len_utf16)
                    .sum();
                match client.completion(editor.row, col_utf16) {
                    Ok(items) => workspace.active_mut().show_completions(items),
                    Err(e) => workspace.active_mut().message = format!("LSP: {e}"),
                }
            } else {
                workspace.active_mut().message = "No language server for this file".into()
            }
            continue;
        }
        if workspace.active().mode == Mode::Normal {
            let body = size().0.saturating_sub(3).max(1);
            let scroll = match key {
                Key::Ctrl('d') => Some((true, (body / 2).max(1))),
                Key::Ctrl('u') => Some((false, (body / 2).max(1))),
                Key::Ctrl('f') => Some((true, body)),
                Key::Ctrl('b') => Some((false, body)),
                _ => None,
            };
            if let Some((down, lines)) = scroll {
                workspace.active_mut().scroll_rows(down, lines);
                continue;
            }
        }
        workspace.handle(key);
        if workspace.active().request_hover {
            workspace.active_mut().request_hover = false;
            if let Some(client) = lsp.as_mut() {
                let editor = workspace.active();
                let col_utf16 = editor.lines[editor.row]
                    .chars()
                    .take(editor.col)
                    .map(char::len_utf16)
                    .sum();
                match client.hover(editor.row, col_utf16) {
                    Ok(Some(text)) => workspace.active_mut().message = text,
                    Ok(None) => workspace.active_mut().message = "No hover information".into(),
                    Err(e) => workspace.active_mut().message = format!("LSP: {e}"),
                }
            } else {
                workspace.active_mut().message = "No language server for this file".into()
            }
        }
        if workspace.active().request_definition {
            workspace.active_mut().request_definition = false;
            if let Some(client) = lsp.as_mut() {
                let editor = workspace.active();
                let col_utf16 = editor.lines[editor.row]
                    .chars()
                    .take(editor.col)
                    .map(char::len_utf16)
                    .sum();
                match client.definition(editor.row, col_utf16) {
                    Ok(Some((path, row, col))) => {
                        if let Err(e) = workspace.open_location(path, row, col) {
                            workspace.active_mut().message = e.to_string()
                        }
                    }
                    Ok(None) => workspace.active_mut().message = "Definition not found".into(),
                    Err(e) => workspace.active_mut().message = format!("LSP: {e}"),
                }
            } else {
                workspace.active_mut().message = "No language server for this file".into()
            }
        }
        if workspace.active().request_references {
            workspace.active_mut().request_references = false;
            if let Some(client) = lsp.as_mut() {
                let editor = workspace.active();
                let col_utf16 = editor.lines[editor.row]
                    .chars()
                    .take(editor.col)
                    .map(char::len_utf16)
                    .sum();
                match client.references(editor.row, col_utf16) {
                    Ok(items) if items.is_empty() => {
                        workspace.active_mut().message = "References not found".into()
                    }
                    Ok(items) => workspace.active_mut().show_locations(items, "references"),
                    Err(e) => workspace.active_mut().message = format!("LSP: {e}"),
                }
            } else {
                workspace.active_mut().message = "No language server for this file".into()
            }
        }
    }
    if workspace.forced_quit {
        workspace.discard_modified_journals()?;
    } else {
        workspace.persist_journals()?;
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("ETOT: {e}");
        std::process::exit(1)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tab_cells_follow_four_column_stops() {
        assert_eq!(visual_width("a\tb".chars()), 5);
        assert_eq!(visual_width("abc\t界".chars()), 6);
    }
    #[test]
    fn horizontal_viewport_keeps_cursor_visible_and_clips_cells() {
        let mut left = 0;
        keep_cursor_visible(&mut left, 8, 5);
        assert_eq!(left, 4);
        keep_cursor_visible(&mut left, 3, 5);
        assert_eq!(left, 3);
        assert_eq!(visible_char('\t', 3, 4, 3), None);
        assert_eq!(visible_char('界', 4, 5, 2), Some(" ".into()));
        assert_eq!(visible_char('Z', 6, 5, 2), Some("Z".into()));
        let mut config = Config::default();
        config.ui.line_numbers = false;
        let mut editor = etot_rs::Editor::open(None, false).unwrap();
        editor.lines = vec!["0123456789".into()];
        let line = pane_render_line(
            &editor,
            0,
            &PaneLineContext {
                cursor_row: 0,
                width: 5,
                left: 5,
                config: &config,
                diagnostics: &[],
                syntax_spans: &[],
                search: None,
                active_pane: true,
            },
        );
        assert!(line.contains("56789"));
        assert!(!line.contains("01234"));
    }
    #[test]
    fn contextual_hints_follow_parser_and_ex_prefix() {
        let mut workspace = Workspace::open(None, false).unwrap();
        assert!(hint_view(&workspace).is_none());
        workspace.handle(Key::Char('d'));
        let hint = hint_view(&workspace).unwrap();
        assert_eq!(hint.title, "Elimina");
        let mut output = String::new();
        let config = Config::default();
        append_hint_overlay(&mut output, &workspace, &config, 100, 24);
        assert!(output.contains("Elimina"));
        assert!(output.contains("oggetto"));
        workspace.handle(Key::Esc);
        workspace.handle(Key::Char(':'));
        for c in "Lsp".chars() {
            workspace.handle(Key::Char(c));
        }
        let hint = hint_view(&workspace).unwrap();
        assert_eq!(hint.title, "Comandi");
        assert!(hint.choices.iter().all(|(name, _)| name.starts_with("Lsp")));
        assert_eq!(ex_matches("vsp"), ["vsplit"]);
        assert_eq!(ex_matches("LsPr"), ["LspRename"]);
        assert_eq!(
            completion_matches("set back"),
            ["set background=dark", "set background=light"]
        );
        let config = Config::default();
        let cursor = append_command_popup(&mut output, &workspace, &config, 100, 24);
        assert_eq!(cursor.map(|position| position.0), Some(5));
        assert!(output.contains("Cmdline"));
        assert!(output.contains(":Lsp"));
        let mut suggestions = String::new();
        append_hint_overlay(&mut suggestions, &workspace, &config, 100, 24);
        assert!(suggestions.contains("\x1b[7;19H"));
        assert!(suggestions.contains("LspFormat"));
        append_statusline(&mut output, &workspace, &config, &[], 100, 24);
        assert!(output.contains("\x1b[24;1H"));
        assert!(output.contains(" COMMAND "));
        assert!(output.contains(""));
        assert!(output.contains(" --:--"));
    }
    #[test]
    fn dollar_commands_appear_below_top_cmdline() {
        let mut workspace = Workspace::open(None, false).unwrap();
        workspace.active_mut().lines = vec!["abc".into()];
        workspace.handle(Key::Char('$'));
        assert_eq!(workspace.active().col, 0);
        let config = Config::default();
        let mut popup = String::new();
        assert!(append_command_popup(&mut popup, &workspace, &config, 100, 24).is_none());
        assert!(popup.contains("Cmdline"));
        assert!(popup.contains('$'));
        assert!(!popup.contains(":$"));
        let mut hints = String::new();
        append_hint_overlay(&mut hints, &workspace, &config, 100, 24);
        assert!(hints.contains("\x1b[7;19H"));
        assert!(hints.contains("$end"));
        assert!(hints.contains("$quit"));

        workspace.handle(Key::Char('e'));
        assert_eq!(workspace.active().mode, Mode::Command);
        popup.clear();
        assert!(append_command_popup(&mut popup, &workspace, &config, 100, 24).is_some());
        assert!(popup.contains("$e"));
    }
    #[test]
    fn completion_popup_shows_selection_without_replacing_buffer() {
        let mut editor = etot_rs::Editor::open(None, false).unwrap();
        editor.lines = vec!["pri".into()];
        editor.show_completions(vec!["print".into(), "println".into()]);
        editor.handle(Key::Down);
        let mut output = String::new();
        append_completion_popup(&mut output, &editor, 80, 24, (3, 8));
        assert!(output.contains("Suggerimenti 2/2"));
        assert!(output.contains("> println"));
        assert_eq!(editor.lines[0], "pri");
        let mut narrow = String::new();
        append_completion_popup(&mut narrow, &editor, 24, 10, (3, 8));
        assert!(narrow.is_empty());
    }
}
