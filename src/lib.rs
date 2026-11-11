pub mod clipboard;
pub mod commands;
pub mod config;
pub mod datetime;
pub mod git;
pub mod lsp;
pub mod state;
pub mod syntax;
pub mod terminal;
pub mod tooling;
pub mod workspace;
use commands::{Action, CommandEngine, CommandHint, Motion, Operator};
use git::ChangedFile;
use lsp::{CodeAction, TextEdit};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    OperatorPending,
    Visual,
    VisualLine,
    VisualBlock,
    Replace,
    ReplaceOne,
    Insert,
    Search(bool),
    Command,
    Explorer,
    Picker,
    Quickfix,
    GitStatus,
    GitDiff,
    Completion,
    CodeActions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Ctrl(char),
}
#[derive(Debug, Clone, Copy)]
struct SearchHit {
    position: (usize, usize),
    index: usize,
    total: usize,
}
struct SubstituteSpec {
    lines: Vec<usize>,
    pattern: String,
    replacement: String,
    global: bool,
    case_insensitive: Option<bool>,
    suppress_missing: bool,
    count_only: bool,
}
struct GlobalSpec {
    lines: std::ops::Range<usize>,
    pattern: String,
    inverse: bool,
    operation: String,
}
#[derive(Clone)]
struct StoredRegister {
    lines: Vec<String>,
    linewise: bool,
    blockwise: bool,
}
#[derive(Deserialize)]
struct BufferJournal {
    version: u8,
    path: PathBuf,
    disk_hash: Option<u64>,
    lines: Vec<String>,
    row: usize,
    col: usize,
    trailing_newline: bool,
    modified: bool,
    undo: Vec<(Vec<String>, usize, usize, bool)>,
    redo: Vec<(Vec<String>, usize, usize, bool)>,
}
#[derive(Serialize)]
struct BufferJournalView<'a> {
    version: u8,
    path: PathBuf,
    disk_hash: Option<u64>,
    lines: &'a [String],
    row: usize,
    col: usize,
    trailing_newline: bool,
    modified: bool,
    undo: &'a [(Vec<String>, usize, usize, bool)],
    redo: &'a [(Vec<String>, usize, usize, bool)],
}
impl StoredRegister {
    fn text(&self) -> String {
        let mut text = self.lines.join("\n");
        if self.linewise {
            text.push('\n');
        }
        text
    }
    fn from_text(text: &str) -> Self {
        let linewise = text.ends_with('\n');
        let content = text.strip_suffix('\n').unwrap_or(text);
        Self {
            lines: content.split('\n').map(str::to_owned).collect(),
            linewise,
            blockwise: false,
        }
    }
}

#[derive(Clone)]
pub struct Editor {
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize,
    replace_count: usize,
    viewport_top: usize,
    viewport_height: usize,
    pub mode: Mode,
    pub prompt: String,
    pub message: String,
    pub search: Option<(String, bool)>, // pattern, true = forward
    pub highlight: bool,
    search_origin: Option<(usize, usize)>,
    search_before: Option<(String, bool)>,
    highlight_before: bool,
    pub modified: bool,
    pub last_saved_at: Option<SystemTime>,
    pub revision: u64,
    pub readonly: bool,
    pub ignorecase: bool,
    pub smartcase: bool,
    pub autoindent: bool,
    pub shiftwidth: usize,
    pub quit: bool,
    pub request_definition: bool,
    pub request_references: bool,
    pub request_hover: bool,
    pub path: Option<PathBuf>,
    commands: CommandEngine,
    pending_change: Vec<Key>,
    last_change: Option<Vec<Key>>,
    recording_change: bool,
    insert_once: bool,
    recording_start_revision: u64,
    replaying_change: bool,
    macros: HashMap<char, Vec<Key>>,
    macro_recording: Option<char>,
    macro_keys: Vec<Key>,
    macro_depth: usize,
    last_played_macro: Option<char>,
    marks: HashMap<char, (usize, usize)>,
    jump_back: Vec<(usize, usize)>,
    jump_forward: Vec<(usize, usize)>,
    change_list: Vec<(usize, usize)>,
    change_index: usize,
    change_candidate: Option<(usize, usize)>,
    register: Vec<String>,
    register_linewise: bool,
    register_blockwise: bool,
    named_registers: HashMap<char, StoredRegister>,
    active_register: Option<char>,
    undo: Vec<(Vec<String>, usize, usize, bool)>,
    undo_bytes: usize,
    redo: Vec<(Vec<String>, usize, usize, bool)>,
    disk_hash: Option<u64>,
    trailing_newline: bool,
    project_root: Option<PathBuf>,
    entries: Vec<PathBuf>,
    picker_files: Vec<PathBuf>,
    pub picker_results: Vec<PathBuf>,
    picker_cursor: (usize, usize),
    picker_previous_mode: Mode,
    pub quickfix: Vec<(PathBuf, usize, usize, String)>,
    pub git_entries: Vec<ChangedFile>,
    pub git_diff_lines: Vec<String>,
    git_previous_cursor: (usize, usize),
    pub completion_items: Vec<String>,
    pub completion_index: usize,
    completion_position: (usize, usize),
    completion_previous_mode: Mode,
    pub code_actions: Vec<CodeAction>,
    code_action_position: (usize, usize),
    visual_anchor: (usize, usize),
}

impl Editor {
    pub fn persist_journal(&self) -> io::Result<()> {
        let Some(path) = self.path.as_ref().filter(|path| !path.is_dir()) else {
            return Ok(());
        };
        self.persist_journal_at(&state::buffer_path(path)?)
    }
    fn persist_journal_at(&self, file: &std::path::Path) -> io::Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let journal = BufferJournalView {
            version: 1,
            path: state::absolute(path)?,
            disk_hash: self.disk_hash,
            lines: &self.lines,
            row: self.row,
            col: self.col,
            trailing_newline: self.trailing_newline,
            modified: self.modified,
            undo: &self.undo,
            redo: &self.redo,
        };
        state::write_atomic(file, &journal)
    }
    pub fn restore_journal(&mut self) -> io::Result<bool> {
        let Some(path) = self.path.as_ref().filter(|path| !path.is_dir()) else {
            return Ok(false);
        };
        self.restore_journal_at(&state::buffer_path(path)?)
    }
    fn restore_journal_at(&mut self, file: &std::path::Path) -> io::Result<bool> {
        let Some(path) = self.path.as_ref() else {
            return Ok(false);
        };
        let journal: BufferJournal = match state::read(file) {
            Ok(journal) => journal,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        if journal.version != 1 || journal.path != state::absolute(path)? {
            return Ok(false);
        }
        if journal.disk_hash != self.disk_hash {
            self.message = "Recovery available, but file changed on disk".into();
            return Ok(false);
        }
        if journal.lines.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "empty recovery buffer",
            ));
        }
        self.lines = journal.lines;
        self.row = journal.row.min(self.lines.len() - 1);
        self.col = journal.col;
        self.trailing_newline = journal.trailing_newline;
        self.modified = journal.modified;
        self.undo = journal.undo;
        self.undo_bytes = self
            .undo
            .iter()
            .flat_map(|state| &state.0)
            .map(String::len)
            .sum();
        self.redo = journal.redo;
        self.clamp();
        if self.modified {
            self.message = "Recovered unsaved changes".into();
            self.revision = self.revision.wrapping_add(1);
        }
        Ok(true)
    }
    pub fn discard_journal(&self) -> io::Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        match fs::remove_file(state::buffer_path(path)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
    pub fn journal_signature(&self) -> (u64, bool, Option<u64>) {
        (self.revision, self.modified, self.disk_hash)
    }
    pub fn search_case_insensitive(&self, pattern: &str) -> bool {
        if !self.ignorecase {
            return false;
        }
        if !self.smartcase {
            return true;
        }
        let mut escaped = false;
        for ch in pattern.chars() {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch.is_uppercase() {
                return false;
            }
        }
        true
    }
    pub fn command_hint(&self) -> Option<CommandHint> {
        matches!(self.mode, Mode::Normal | Mode::OperatorPending)
            .then(|| self.commands.hint())
            .flatten()
    }
    pub fn content(&self) -> String {
        let mut text = self.lines.join("\n");
        if self.trailing_newline {
            text.push('\n');
        }
        text
    }
    pub fn apply_lsp_edits(&mut self, edits: &[TextEdit]) -> io::Result<usize> {
        if self.readonly {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Read-only buffer",
            ));
        }
        let mut text = self.content();
        let mut ranges = Vec::with_capacity(edits.len());
        for edit in edits {
            let start = utf16_position_to_byte(&text, edit.start).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "Invalid LSP edit start")
            })?;
            let end = utf16_position_to_byte(&text, edit.end).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "Invalid LSP edit end")
            })?;
            if start > end {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Reversed LSP edit range",
                ));
            }
            ranges.push((start, end, edit.new_text.as_str()));
        }
        ranges.sort_by_key(|range| std::cmp::Reverse(range.0));
        for pair in ranges.windows(2) {
            if pair[1].1 > pair[0].0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Overlapping LSP edits",
                ));
            }
        }
        let original = text.clone();
        for (start, end, replacement) in ranges {
            text.replace_range(start..end, replacement);
        }
        if text == original {
            return Ok(0);
        }
        self.snapshot();
        self.trailing_newline = text.ends_with('\n');
        self.lines = text.split('\n').map(str::to_owned).collect();
        if self.trailing_newline {
            self.lines.pop();
        }
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.row = self.row.min(self.lines.len() - 1);
        self.clamp();
        self.mark_changed();
        Ok(edits.len())
    }
    pub fn open(path: Option<PathBuf>, readonly: bool) -> io::Result<Self> {
        if let Some(dir) = path.as_ref().filter(|p| p.is_dir()) {
            let mut editor = Self::open(None, readonly)?;
            editor.open_directory(dir.clone())?;
            return Ok(editor);
        }
        let original = match path.as_ref() {
            Some(p) if p.exists() => Some(fs::read(p)?),
            _ => None,
        };
        let trailing_newline = original
            .as_ref()
            .is_none_or(|bytes| bytes.last() == Some(&b'\n'));
        let disk_hash = original.as_ref().map(|bytes| hash_bytes(bytes));
        let last_saved_at = path
            .as_ref()
            .and_then(|path| fs::metadata(path).ok()?.modified().ok());
        let project_root = path.as_ref().and_then(|path| detect_project_root(path));
        let lines = match original {
            Some(bytes) => String::from_utf8(bytes)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "file is not UTF-8"))?
                .lines()
                .map(str::to_owned)
                .collect(),
            None => Vec::new(),
        };
        Ok(Self {
            lines: if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            },
            row: 0,
            col: 0,
            replace_count: 1,
            viewport_top: 0,
            viewport_height: 1,
            mode: Mode::Normal,
            prompt: String::new(),
            message: String::new(),
            search: None,
            highlight: false,
            search_origin: None,
            search_before: None,
            highlight_before: false,
            modified: false,
            last_saved_at,
            revision: 0,
            readonly,
            ignorecase: false,
            smartcase: false,
            autoindent: false,
            shiftwidth: 4,
            quit: false,
            request_definition: false,
            request_references: false,
            request_hover: false,
            path,
            commands: CommandEngine::default(),
            pending_change: Vec::new(),
            last_change: None,
            recording_change: false,
            insert_once: false,
            recording_start_revision: 0,
            replaying_change: false,
            macros: HashMap::new(),
            macro_recording: None,
            macro_keys: Vec::new(),
            macro_depth: 0,
            last_played_macro: None,
            marks: HashMap::new(),
            jump_back: Vec::new(),
            jump_forward: Vec::new(),
            change_list: Vec::new(),
            change_index: 0,
            change_candidate: None,
            register: Vec::new(),
            register_linewise: true,
            register_blockwise: false,
            named_registers: HashMap::new(),
            active_register: None,
            undo: Vec::new(),
            undo_bytes: 0,
            redo: Vec::new(),
            disk_hash,
            trailing_newline,
            project_root,
            entries: Vec::new(),
            picker_files: Vec::new(),
            picker_results: Vec::new(),
            picker_cursor: (0, 0),
            picker_previous_mode: Mode::Normal,
            quickfix: Vec::new(),
            git_entries: Vec::new(),
            git_diff_lines: Vec::new(),
            git_previous_cursor: (0, 0),
            completion_items: Vec::new(),
            completion_index: 0,
            completion_position: (0, 0),
            completion_previous_mode: Mode::Normal,
            code_actions: Vec::new(),
            code_action_position: (0, 0),
            visual_anchor: (0, 0),
        })
    }
    fn open_directory(&mut self, dir: PathBuf) -> io::Result<()> {
        let mut entries: Vec<PathBuf> = fs::read_dir(&dir)?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .collect();
        entries.sort_by_key(|p| (!p.is_dir(), p.file_name().map(|n| n.to_os_string())));
        self.lines = entries
            .iter()
            .map(|p| {
                let mut name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if p.is_dir() {
                    name.push('/');
                    format!(">  {name}")
                } else {
                    format!("-  {name}")
                }
            })
            .collect();
        if self.lines.is_empty() {
            self.lines.push("[Empty directory]".into())
        }
        self.entries = entries;
        self.row = 0;
        self.col = 0;
        self.mode = Mode::Explorer;
        self.project_root = Some(dir);
        Ok(())
    }
    fn explorer_key(&mut self, key: Key) {
        match key {
            Key::Char('j') | Key::Down => self.move_vertical(true),
            Key::Char('k') | Key::Up => self.move_vertical(false),
            Key::Enter => {
                if let Some(path) = self.entries.get(self.row).cloned() {
                    if path.is_dir() {
                        if let Err(e) = self.open_directory(path) {
                            self.message = e.to_string()
                        }
                    } else {
                        let root = self.project_root.clone();
                        match Self::open(Some(path), self.readonly) {
                            Ok(mut file) => {
                                file.project_root = root;
                                *self = file
                            }
                            Err(e) => self.message = e.to_string(),
                        }
                    }
                }
            }
            Key::Backspace | Key::Char('-') => {
                if let Some(parent) = self
                    .project_root
                    .as_ref()
                    .and_then(|d| d.parent())
                    .map(PathBuf::from)
                {
                    if let Err(e) = self.open_directory(parent) {
                        self.message = e.to_string()
                    }
                }
            }
            Key::Char('q') | Key::Esc => self.quit = true,
            _ => (),
        }
    }
    fn collect_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
        if out.len() >= 20_000 {
            return Ok(());
        }
        let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let name = entry.file_name();
            if [".git", "target", "node_modules", ".deps", "build"]
                .iter()
                .any(|skip| name == *skip)
            {
                continue;
            }
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                Self::collect_files(&path, out)?
            } else if file_type.is_file() {
                out.push(path)
            }
            if out.len() >= 20_000 {
                break;
            }
        }
        Ok(())
    }
    fn open_picker(&mut self) {
        let root = self
            .project_root
            .clone()
            .or_else(|| {
                self.path
                    .as_ref()
                    .and_then(|p| p.parent())
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| PathBuf::from("."));
        self.picker_files.clear();
        if let Err(e) = Self::collect_files(&root, &mut self.picker_files) {
            self.message = e.to_string();
            return;
        }
        self.picker_results = self.picker_files.iter().take(100).cloned().collect();
        self.project_root = Some(root);
        self.picker_cursor = (self.row, self.col);
        self.picker_previous_mode = self.mode;
        self.row = 0;
        self.col = 0;
        self.prompt.clear();
        self.mode = Mode::Picker;
    }
    fn refresh_picker(&mut self) {
        let needle = self.prompt.to_lowercase();
        self.picker_results = self
            .picker_files
            .iter()
            .filter(|p| {
                let relative = p
                    .strip_prefix(
                        self.project_root
                            .as_deref()
                            .unwrap_or_else(|| std::path::Path::new(".")),
                    )
                    .unwrap_or(p);
                let mut chars = relative
                    .to_string_lossy()
                    .to_lowercase()
                    .chars()
                    .collect::<Vec<_>>()
                    .into_iter();
                needle
                    .chars()
                    .all(|c| chars.by_ref().any(|candidate| candidate == c))
            })
            .take(100)
            .cloned()
            .collect();
        self.row = self.row.min(self.picker_results.len().saturating_sub(1));
    }
    fn picker_key(&mut self, key: Key) {
        match key {
            Key::Esc => {
                self.mode = Mode::Normal;
                (self.row, self.col) = self.picker_cursor
            }
            Key::Down | Key::Ctrl('n') => {
                self.row = (self.row + 1).min(self.picker_results.len().saturating_sub(1))
            }
            Key::Up | Key::Ctrl('p') => self.row = self.row.saturating_sub(1),
            Key::Backspace => {
                self.prompt.pop();
                self.refresh_picker()
            }
            Key::Char(c) => {
                self.prompt.push(c);
                self.refresh_picker()
            }
            Key::Enter => {
                if let Some(path) = self.picker_results.get(self.row).cloned() {
                    let root = self.project_root.clone();
                    match Self::open(Some(path), self.readonly) {
                        Ok(mut file) => {
                            file.project_root = root;
                            *self = file
                        }
                        Err(e) => self.message = e.to_string(),
                    }
                }
            }
            _ => (),
        }
    }
    fn grep_project(&mut self, pattern: &str) {
        let regex = match Regex::new(pattern) {
            Ok(regex) => regex,
            Err(e) => {
                self.message = e.to_string();
                return;
            }
        };
        let root = self
            .project_root
            .clone()
            .or_else(|| {
                self.path
                    .as_ref()
                    .and_then(|p| p.parent())
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| PathBuf::from("."));
        let mut files = Vec::new();
        if let Err(e) = Self::collect_files(&root, &mut files) {
            self.message = e.to_string();
            return;
        }
        self.quickfix.clear();
        for path in files {
            if let Ok(content) = fs::read_to_string(&path) {
                for (line_number, line) in content.lines().enumerate() {
                    if let Some(found) = regex.find(line) {
                        self.quickfix.push((
                            path.clone(),
                            line_number,
                            line[..found.start()].chars().count(),
                            line.trim().to_owned(),
                        ));
                        if self.quickfix.len() >= 1000 {
                            break;
                        }
                    }
                }
            }
            if self.quickfix.len() >= 1000 {
                break;
            }
        }
        self.project_root = Some(root);
        self.picker_cursor = (self.row, self.col);
        self.picker_previous_mode = self.mode;
        self.row = 0;
        self.col = 0;
        let found = self.quickfix.len();
        self.message = format!("{found} matches");
        self.mode = Mode::Quickfix;
    }
    fn quickfix_key(&mut self, key: Key) {
        match key {
            Key::Char('j') | Key::Down => {
                self.row = (self.row + 1).min(self.quickfix.len().saturating_sub(1))
            }
            Key::Char('k') | Key::Up => self.row = self.row.saturating_sub(1),
            Key::Esc | Key::Char('q') => {
                self.mode = Mode::Normal;
                (self.row, self.col) = self.picker_cursor
            }
            Key::Enter => {
                if let Some((path, row, col, _)) = self.quickfix.get(self.row).cloned() {
                    let root = self.project_root.clone();
                    match Self::open(Some(path), self.readonly) {
                        Ok(mut file) => {
                            file.project_root = root;
                            file.row = row;
                            file.col = col;
                            *self = file
                        }
                        Err(e) => self.message = e.to_string(),
                    }
                }
            }
            _ => (),
        }
    }
    pub fn show_quickfix(&mut self, entries: Vec<(PathBuf, usize, usize, String)>, label: &str) {
        self.quickfix = entries;
        self.picker_cursor = (self.row, self.col);
        self.picker_previous_mode = self.mode;
        self.row = 0;
        self.col = 0;
        self.mode = Mode::Quickfix;
        self.message = format!("{} {label}", self.quickfix.len());
    }
    pub fn show_locations(&mut self, locations: Vec<(PathBuf, usize, usize)>, label: &str) {
        self.show_quickfix(
            locations
                .into_iter()
                .map(|(path, row, col)| (path, row, col, label.to_owned()))
                .collect(),
            label,
        );
    }
    pub fn before_navigation(&self) -> Self {
        let mut previous = self.clone();
        if matches!(previous.mode, Mode::Picker | Mode::Quickfix) {
            previous.mode = previous.picker_previous_mode;
            (previous.row, previous.col) = previous.picker_cursor;
        } else if previous.mode == Mode::GitStatus {
            previous.mode = Mode::Normal;
            (previous.row, previous.col) = previous.git_previous_cursor;
        }
        previous
    }
    fn git_cwd(&self) -> PathBuf {
        self.project_root
            .clone()
            .or_else(|| {
                self.path
                    .as_ref()
                    .and_then(|path| path.parent())
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| PathBuf::from("."))
    }
    fn open_git_status(&mut self) {
        match git::status(&self.git_cwd()) {
            Ok(entries) => {
                self.git_entries = entries;
                self.git_previous_cursor = (self.row, self.col);
                self.row = 0;
                self.col = 0;
                self.mode = Mode::GitStatus;
                self.message = format!(
                    "{} changed files (s: stage, Enter: open, q: close)",
                    self.git_entries.len()
                );
            }
            Err(e) => self.message = format!("Git: {e}"),
        }
    }
    fn git_status_key(&mut self, key: Key) {
        match key {
            Key::Char('j') | Key::Down => {
                self.row = (self.row + 1).min(self.git_entries.len().saturating_sub(1))
            }
            Key::Char('k') | Key::Up => self.row = self.row.saturating_sub(1),
            Key::Char('q') | Key::Esc => {
                self.mode = Mode::Normal;
                (self.row, self.col) = self.git_previous_cursor
            }
            Key::Char('s') => {
                if let Some(entry) = self.git_entries.get(self.row) {
                    match git::stage(&self.git_cwd(), &entry.path) {
                        Ok(()) => match git::status(&self.git_cwd()) {
                            Ok(entries) => {
                                self.git_entries = entries;
                                self.row = self.row.min(self.git_entries.len().saturating_sub(1));
                                self.message = "Staged".into()
                            }
                            Err(e) => self.message = e.to_string(),
                        },
                        Err(e) => self.message = e.to_string(),
                    }
                }
            }
            Key::Enter => {
                if let Some(entry) = self.git_entries.get(self.row) {
                    match Self::open(Some(entry.path.clone()), self.readonly) {
                        Ok(mut editor) => {
                            editor.project_root = self.project_root.clone();
                            *self = editor
                        }
                        Err(e) => self.message = e.to_string(),
                    }
                }
            }
            _ => (),
        }
    }
    fn open_git_diff(&mut self) {
        let Some(path) = self.path.as_ref() else {
            self.message = "No file for Git diff".into();
            return;
        };
        match git::diff(&self.git_cwd(), path) {
            Ok(lines) => {
                self.git_diff_lines = if lines.is_empty() {
                    vec!["[No unstaged diff]".into()]
                } else {
                    lines
                };
                self.git_previous_cursor = (self.row, self.col);
                self.row = 0;
                self.col = 0;
                self.mode = Mode::GitDiff;
            }
            Err(e) => self.message = format!("Git: {e}"),
        }
    }
    pub fn show_completions(&mut self, items: Vec<String>) {
        if self.readonly {
            self.message = "Read-only buffer".into();
            return;
        }
        if items.is_empty() {
            self.message = "No completions".into();
            return;
        }
        self.completion_items = items;
        self.completion_index = 0;
        self.completion_previous_mode = self.mode;
        self.completion_position = (self.row, self.col);
        self.mode = Mode::Completion;
    }
    fn completion_key(&mut self, key: Key) {
        match key {
            Key::Down | Key::Ctrl('n') | Key::Ctrl('i') => {
                self.completion_index = (self.completion_index + 1) % self.completion_items.len()
            }
            Key::Up | Key::Ctrl('p') => {
                self.completion_index = (self.completion_index + self.completion_items.len() - 1)
                    % self.completion_items.len()
            }
            Key::Esc => {
                self.mode = self.completion_previous_mode;
                (self.row, self.col) = self.completion_position
            }
            Key::Char(c) if self.completion_previous_mode == Mode::Insert => {
                self.mode = Mode::Insert;
                (self.row, self.col) = self.completion_position;
                self.insert(c);
            }
            Key::Backspace if self.completion_previous_mode == Mode::Insert => {
                self.mode = Mode::Insert;
                (self.row, self.col) = self.completion_position;
                self.backspace();
            }
            Key::Enter => {
                let chosen = self.completion_items[self.completion_index].clone();
                self.mode = self.completion_previous_mode;
                (self.row, self.col) = self.completion_position;
                let chars = self.chars();
                let end = if self.mode == Mode::Insert {
                    self.col
                } else {
                    (self.col + 1).min(chars.len())
                };
                let mut start = end;
                while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
                    start -= 1
                }
                let prefix: String = chars[..start].iter().collect();
                let suffix: String = chars[end..].iter().collect();
                self.snapshot();
                self.lines[self.row] = format!("{prefix}{chosen}{suffix}");
                self.col = start + chosen.chars().count();
                if self.mode == Mode::Normal {
                    self.col = self.col.saturating_sub(1)
                }
                self.mark_changed();
            }
            _ => (),
        }
    }
    pub fn show_code_actions(&mut self, actions: Vec<CodeAction>) {
        if actions.is_empty() {
            self.message = "No code actions".into();
            return;
        }
        self.code_actions = actions;
        self.code_action_position = (self.row, self.col);
        self.row = 0;
        self.col = 0;
        self.mode = Mode::CodeActions;
    }
    fn code_action_key(&mut self, key: Key) {
        match key {
            Key::Down | Key::Char('j') => {
                self.row = (self.row + 1).min(self.code_actions.len().saturating_sub(1));
            }
            Key::Up | Key::Char('k') => self.row = self.row.saturating_sub(1),
            Key::Esc | Key::Char('q') => {
                self.mode = Mode::Normal;
                (self.row, self.col) = self.code_action_position;
            }
            Key::Enter => {
                let action = self.code_actions.get(self.row).cloned();
                self.mode = Mode::Normal;
                (self.row, self.col) = self.code_action_position;
                if let Some(action) = action {
                    self.message = match self.apply_lsp_edits(&action.edits) {
                        Ok(_) => format!("Applied: {}", action.title),
                        Err(e) => format!("Code action: {e}"),
                    };
                }
            }
            _ => (),
        }
    }
    pub fn project_root_path(&self) -> Option<&std::path::Path> {
        self.project_root.as_deref()
    }
    pub fn begin_insert(&mut self) {
        if self.readonly {
            self.message = "Read-only buffer".into();
            return;
        }
        self.snapshot();
        self.mode = Mode::Insert;
    }
    pub fn selection_contains(&self, row: usize, col: usize) -> bool {
        let (anchor_row, anchor_col) = self.visual_anchor;
        match self.mode {
            Mode::VisualLine => row >= anchor_row.min(self.row) && row <= anchor_row.max(self.row),
            Mode::VisualBlock => {
                row >= anchor_row.min(self.row)
                    && row <= anchor_row.max(self.row)
                    && col >= anchor_col.min(self.col)
                    && col <= anchor_col.max(self.col)
            }
            Mode::Visual => {
                (row, col) >= (anchor_row, anchor_col).min((self.row, self.col))
                    && (row, col) <= (anchor_row, anchor_col).max((self.row, self.col))
            }
            _ => false,
        }
    }
    fn visual_action(&mut self, op: Operator) {
        if self.readonly && op != Operator::Yank {
            self.message = "Read-only buffer".into();
            self.mode = Mode::Normal;
            return;
        }
        let (anchor_row, anchor_col) = self.visual_anchor;
        let (start_row, end_row) = (anchor_row.min(self.row), anchor_row.max(self.row));
        if self.mode == Mode::VisualLine {
            self.row = start_row;
            self.mode = Mode::Normal;
            self.operate_lines(op, end_row - start_row + 1);
            return;
        }
        if self.mode == Mode::VisualBlock {
            let (left, right) = (anchor_col.min(self.col), anchor_col.max(self.col));
            self.register = self.lines[start_row..=end_row]
                .iter()
                .map(|line| line.chars().skip(left).take(right - left + 1).collect())
                .collect();
            self.register_linewise = false;
            self.register_blockwise = true;
            self.store_register(op == Operator::Yank);
            if op != Operator::Yank {
                self.snapshot();
                for row in start_row..=end_row {
                    let mut chars: Vec<char> = self.lines[row].chars().collect();
                    let end = (right + 1).min(chars.len());
                    if left < end {
                        chars.drain(left..end);
                        self.lines[row] = chars.into_iter().collect()
                    }
                }
                self.mark_changed();
            }
            self.row = start_row;
            self.col = left.min(self.chars().len().saturating_sub(1));
        } else {
            let (start, end) = if (anchor_row, anchor_col) <= (self.row, self.col) {
                ((anchor_row, anchor_col), (self.row, self.col))
            } else {
                ((self.row, self.col), (anchor_row, anchor_col))
            };
            let first: String = self.lines[start.0].chars().skip(start.1).collect();
            let last: String = self.lines[end.0].chars().take(end.1 + 1).collect();
            self.register = if start.0 == end.0 {
                vec![first.chars().take(end.1 - start.1 + 1).collect()]
            } else {
                let mut selected = vec![first];
                selected.extend(self.lines[start.0 + 1..end.0].iter().cloned());
                selected.push(last);
                selected
            };
            self.register_linewise = false;
            self.register_blockwise = false;
            self.store_register(op == Operator::Yank);
            if op != Operator::Yank {
                (self.row, self.col) = start;
                self.snapshot();
                let prefix: String = self.lines[start.0].chars().take(start.1).collect();
                let suffix: String = self.lines[end.0].chars().skip(end.1 + 1).collect();
                self.lines
                    .splice(start.0..=end.0, [format!("{prefix}{suffix}")]);
                self.mark_changed();
            }
            self.row = start.0;
            self.col = start.1.min(self.chars().len().saturating_sub(1));
        }
        self.mode = if op == Operator::Change {
            Mode::Insert
        } else {
            Mode::Normal
        };
    }
    fn visual_key(&mut self, key: Key) {
        match key {
            Key::Esc | Key::Char('v') if self.mode == Mode::Visual => self.mode = Mode::Normal,
            Key::Esc | Key::Char('V') if self.mode == Mode::VisualLine => self.mode = Mode::Normal,
            Key::Esc => self.mode = Mode::Normal,
            Key::Char('d') | Key::Delete | Key::Backspace => self.visual_action(Operator::Delete),
            Key::Char('y') => self.visual_action(Operator::Yank),
            Key::Char('c') => self.visual_action(Operator::Change),
            Key::Char('>') | Key::Char('<') => {
                let first = self.visual_anchor.0.min(self.row);
                let last = self.visual_anchor.0.max(self.row);
                self.indent_rows(first, last, key == Key::Char('>'));
                self.mode = Mode::Normal;
            }
            Key::Char('h') | Key::Left => self.step(Motion::Left),
            Key::Char('j') | Key::Down => self.step(Motion::Down),
            Key::Char('k') | Key::Up => self.step(Motion::Up),
            Key::Char('l') | Key::Right => self.step(Motion::Right),
            Key::Char('0') | Key::Home => self.step(Motion::Start),
            Key::End => self.step(Motion::End),
            Key::Char('w') => self.step(Motion::Word),
            Key::Char('b') => self.step(Motion::BackWord),
            _ => (),
        }
    }
    fn replace_key(&mut self, key: Key) {
        match key {
            Key::Home => self.step(Motion::Start),
            Key::End => self.step(Motion::End),
            Key::Esc => {
                self.mode = Mode::Normal;
                self.clamp()
            }
            Key::Char(c) => {
                let mut chars = self.chars();
                if self.mode == Mode::ReplaceOne {
                    let end = self.col.saturating_add(self.replace_count);
                    if end > chars.len() {
                        self.mode = Mode::Normal;
                        self.message = "Not enough characters for r".into();
                        return;
                    }
                    self.snapshot();
                    for current in &mut chars[self.col..end] {
                        *current = c;
                    }
                    self.col = end - 1;
                } else if self.col < chars.len() {
                    chars[self.col] = c;
                } else {
                    chars.push(c);
                }
                self.lines[self.row] = chars.into_iter().collect();
                self.mark_changed();
                if self.mode == Mode::ReplaceOne {
                    self.mode = Mode::Normal
                } else {
                    self.col += 1
                }
            }
            Key::Enter if self.mode == Mode::ReplaceOne => {
                let chars = self.chars();
                let end = self.col.saturating_add(self.replace_count);
                if end > chars.len() {
                    self.message = "Not enough characters for r".into();
                } else {
                    self.snapshot();
                    let first: String = chars[..self.col].iter().collect();
                    let second: String = chars[end..].iter().collect();
                    self.lines.splice(self.row..=self.row, [first, second]);
                    self.row += 1;
                    self.col = 0;
                    self.mark_changed();
                }
                self.mode = Mode::Normal;
            }
            Key::Enter if self.mode == Mode::Replace => self.enter(),
            Key::Left if self.mode == Mode::Replace => self.col = self.col.saturating_sub(1),
            Key::Right if self.mode == Mode::Replace => {
                self.col = (self.col + 1).min(self.chars().len())
            }
            _ => (),
        }
    }
    fn chars(&self) -> Vec<char> {
        self.lines[self.row].chars().collect()
    }
    fn clamp(&mut self) {
        self.col = self.col.min(self.chars().len().saturating_sub(1));
    }
    fn move_vertical(&mut self, down: bool) {
        self.row = if down {
            (self.row + 1).min(self.lines.len() - 1)
        } else {
            self.row.saturating_sub(1)
        };
        self.clamp();
    }
    pub fn scroll_rows(&mut self, down: bool, count: usize) {
        self.row = if down {
            self.row.saturating_add(count).min(self.lines.len() - 1)
        } else {
            self.row.saturating_sub(count)
        };
        self.clamp();
    }
    pub fn set_viewport(&mut self, top: usize, height: usize) {
        self.viewport_top = top;
        self.viewport_height = height.max(1);
    }
    fn move_horizontal(&mut self, right: bool) {
        self.col = if right {
            (self.col + 1).min(self.chars().len().saturating_sub(1))
        } else {
            self.col.saturating_sub(1)
        };
    }
    fn search_next(&mut self, forward: bool) {
        let Some((pattern, _)) = self.search.clone() else {
            return;
        };
        if pattern.is_empty() {
            return;
        }
        let found = match self.search_hit(&pattern, forward, (self.row, self.col)) {
            Ok(found) => found,
            Err(e) => {
                self.message = format!("Invalid pattern: {e}");
                return;
            }
        };
        let Some(SearchHit {
            position,
            index,
            total,
        }) = found
        else {
            self.message = format!("Pattern not found: {pattern}");
            return;
        };
        self.record_jump(position);
        (self.row, self.col) = position;
        self.highlight = true;
        self.message = format!("{index}/{total}");
    }
    fn search_hit(
        &self,
        pattern: &str,
        forward: bool,
        origin: (usize, usize),
    ) -> Result<Option<SearchHit>, regex::Error> {
        let regex = vim_regex(pattern, self.search_case_insensitive(pattern))?;
        let mut first = None;
        let mut last = None;
        let mut next = None;
        let mut previous = None;
        let mut total = 0;
        for (row, line) in self.lines.iter().enumerate() {
            let mut previous_byte = 0;
            let mut character = 0;
            for found in regex.find_iter(line) {
                total += 1;
                character += line[previous_byte..found.start()].chars().count();
                previous_byte = found.start();
                let position = (row, character);
                let hit = (position, total);
                first.get_or_insert(hit);
                last = Some(hit);
                if position > origin && next.is_none() {
                    next = Some(hit);
                }
                if position < origin {
                    previous = Some(hit);
                }
            }
        }
        let selected = if forward {
            next.or(first)
        } else {
            previous.or(last)
        };
        Ok(selected.map(|(position, index)| SearchHit {
            position,
            index,
            total,
        }))
    }
    fn preview_search(&mut self, forward: bool) {
        let origin = self.search_origin.unwrap_or((self.row, self.col));
        if self.prompt.is_empty() {
            (self.row, self.col) = origin;
            self.search = self.search_before.clone();
            self.highlight = self.highlight_before;
            self.message.clear();
            return;
        }
        match self.search_hit(&self.prompt, forward, origin) {
            Ok(Some(SearchHit {
                position,
                index,
                total,
            })) => {
                (self.row, self.col) = position;
                self.search = Some((self.prompt.clone(), forward));
                self.highlight = true;
                self.message = format!("{index}/{total}");
            }
            Ok(None) => {
                (self.row, self.col) = origin;
                self.search = self.search_before.clone();
                self.highlight = self.highlight_before;
                self.message = format!("Pattern not found: {}", self.prompt);
            }
            Err(e) => {
                (self.row, self.col) = origin;
                self.search = self.search_before.clone();
                self.highlight = self.highlight_before;
                self.message = format!("Invalid pattern: {e}");
            }
        }
    }
    fn cycle_search_preview(&mut self, forward: bool) {
        if self.prompt.is_empty() {
            if let Some((pattern, _)) = self.search_before.clone() {
                self.prompt = pattern;
                self.preview_search(forward);
                return;
            } else {
                return;
            }
        }
        if self.search.as_ref().map(|(pattern, _)| pattern.as_str()) != Some(self.prompt.as_str()) {
            return;
        }
        match self.search_hit(&self.prompt, forward, (self.row, self.col)) {
            Ok(Some(hit)) => {
                (self.row, self.col) = hit.position;
                self.message = format!("{}/{}", hit.index, hit.total);
            }
            Ok(None) => self.message = format!("Pattern not found: {}", self.prompt),
            Err(error) => self.message = format!("Invalid pattern: {error}"),
        }
    }
    fn finish_search(&mut self, forward: bool, commit: bool) {
        let origin = self.search_origin.take().unwrap_or((self.row, self.col));
        let previous = self.search_before.take();
        let previous_highlight = self.highlight_before;
        let preview = (self.row, self.col);
        let preview_message = self.message.clone();
        let preview_matches = !self.prompt.is_empty()
            && self.search.as_ref().map(|(pattern, _)| pattern.as_str())
                == Some(self.prompt.as_str())
            && self.highlight;
        self.mode = Mode::Normal;
        if !commit {
            (self.row, self.col) = origin;
            self.search = previous;
            self.highlight = previous_highlight;
            self.prompt.clear();
            self.message.clear();
            return;
        }
        let pattern = if self.prompt.is_empty() {
            previous.as_ref().map(|(pattern, _)| pattern.clone())
        } else {
            Some(self.prompt.clone())
        };
        self.prompt.clear();
        let Some(pattern) = pattern else { return };
        match self.search_hit(&pattern, forward, origin) {
            Ok(Some(SearchHit {
                position,
                index,
                total,
            })) => {
                (self.row, self.col) = origin;
                let selected = if preview_matches { preview } else { position };
                self.record_jump(selected);
                (self.row, self.col) = selected;
                self.search = Some((pattern, forward));
                self.highlight = true;
                self.message = if preview_matches {
                    preview_message
                } else {
                    format!("{index}/{total}")
                };
            }
            Ok(None) => {
                (self.row, self.col) = origin;
                self.search = Some((pattern.clone(), forward));
                self.highlight = true;
                self.message = format!("Pattern not found: {pattern}");
            }
            Err(e) => {
                (self.row, self.col) = origin;
                self.search = previous;
                self.highlight = previous_highlight;
                self.message = format!("Invalid pattern: {e}");
            }
        }
    }
    fn search_word(&mut self, forward: bool) {
        let chars = self.chars();
        if chars.is_empty() {
            return;
        }
        let mut start = self.col.min(chars.len() - 1);
        if !chars[start].is_alphanumeric() && chars[start] != '_' {
            self.message = "No word under cursor".into();
            return;
        }
        let mut end = start + 1;
        while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
            start -= 1;
        }
        while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
            end += 1;
        }
        let word: String = chars[start..end].iter().collect();
        self.search = Some((format!(r"\b{}\b", regex::escape(&word)), forward));
        self.search_next(forward);
    }
    fn record_jump(&mut self, target: (usize, usize)) {
        let current = (self.row, self.col);
        if current != target {
            if self.jump_back.last() != Some(&current) {
                self.jump_back.push(current);
                if self.jump_back.len() > 100 {
                    self.jump_back.remove(0);
                }
            }
            self.jump_forward.clear();
        }
    }
    fn jump_history(&mut self, forward: bool) {
        let (from, to) = if forward {
            (&mut self.jump_forward, &mut self.jump_back)
        } else {
            (&mut self.jump_back, &mut self.jump_forward)
        };
        if let Some(target) = from.pop() {
            to.push((self.row, self.col));
            self.row = target.0.min(self.lines.len().saturating_sub(1));
            self.col = target
                .1
                .min(self.lines[self.row].chars().count().saturating_sub(1));
        } else {
            self.message = "Jump list empty".into();
        }
    }
    fn jump_changes(&mut self, forward: bool, count: usize) {
        if self.change_list.is_empty() {
            self.message = "Change list empty".into();
            return;
        }
        self.change_index = if forward {
            self.change_index
                .saturating_add(count)
                .min(self.change_list.len() - 1)
        } else {
            self.change_index.saturating_sub(count)
        };
        let (row, col) = self.change_list[self.change_index];
        self.record_jump((row, col));
        self.row = row.min(self.lines.len() - 1);
        self.col = col.min(self.lines[self.row].chars().count().saturating_sub(1));
    }
    pub fn save(&mut self) -> io::Result<()> {
        if self.readonly {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "file is read-only",
            ));
        }
        let path = self
            .path
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
        let current = match fs::read(path) {
            Ok(bytes) => Some(hash_bytes(&bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        if current != self.disk_hash {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "file changed externally; save aborted",
            ));
        }
        if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "saving through a symlink is not supported",
            ));
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let output = self.content();
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        if let Ok(meta) = fs::metadata(path) {
            temp.as_file().set_permissions(meta.permissions())?
        }
        temp.write_all(output.as_bytes())?;
        temp.as_file().sync_all()?;
        temp.persist(path).map_err(|e| e.error)?;
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        self.disk_hash = Some(hash_bytes(output.as_bytes()));
        self.modified = false;
        self.last_saved_at = Some(SystemTime::now());
        self.message = format!("{} written", path.display());
        Ok(())
    }
    pub fn save_as(&mut self, target: &std::path::Path, overwrite: bool) -> io::Result<()> {
        if self.readonly {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "file is read-only",
            ));
        }
        let same_path = self.path.as_deref() == Some(target)
            || self
                .path
                .as_deref()
                .and_then(|path| path.canonicalize().ok())
                .zip(target.canonicalize().ok())
                .is_some_and(|(current, other)| current == other);
        if same_path && !overwrite {
            return self.save();
        }
        if fs::symlink_metadata(target).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "saving through a symlink is not supported",
            ));
        }
        if !overwrite && target.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "target already exists",
            ));
        }
        let parent = target
            .parent()
            .filter(|part| !part.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let content = self.content();
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        if let Ok(meta) = fs::metadata(target) {
            if !meta.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "target is not a file",
                ));
            }
            temporary.as_file().set_permissions(meta.permissions())?;
        }
        temporary.write_all(content.as_bytes())?;
        temporary.as_file().sync_all()?;
        if overwrite {
            temporary.persist(target).map_err(|error| error.error)?;
        } else {
            temporary
                .persist_noclobber(target)
                .map_err(|error| error.error)?;
        }
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
        self.last_saved_at = Some(SystemTime::now());
        if self.path.is_none() || same_path {
            self.path = Some(target.to_path_buf());
            self.disk_hash = Some(hash_bytes(content.as_bytes()));
            self.project_root = detect_project_root(target);
            self.modified = false;
            self.message = format!("{} written", target.display());
        } else {
            self.message = format!("Copy saved to {}", target.display());
        }
        Ok(())
    }
    pub fn copy_visual_selection(&mut self) {
        if matches!(
            self.mode,
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock
        ) {
            let previous_register = self.active_register.replace('+');
            self.visual_action(Operator::Yank);
            self.active_register = previous_register;
        }
    }
    fn insert(&mut self, c: char) {
        let mut chars = self.chars();
        chars.insert(self.col.min(chars.len()), c);
        self.lines[self.row] = chars.into_iter().collect();
        self.col += 1;
        self.mark_changed();
        if matches!(c, 'e' | 'S') {
            let prefix = self.lines[self.row]
                .chars()
                .take(self.col)
                .collect::<Vec<_>>();
            let mut word = prefix
                .iter()
                .rev()
                .take_while(|ch| ch.is_alphanumeric() || **ch == '_')
                .copied()
                .collect::<Vec<_>>();
            word.reverse();
            if let Some(value) = datetime::suggestion(&word.into_iter().collect::<String>()) {
                self.show_completions(vec![value]);
            }
        }
    }
    fn backspace(&mut self) {
        if self.col > 0 {
            let mut chars = self.chars();
            self.col -= 1;
            chars.remove(self.col);
            self.lines[self.row] = chars.into_iter().collect();
            self.mark_changed();
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
            self.lines[self.row].push_str(&line);
            self.mark_changed();
        }
    }
    fn delete_insert(&mut self) {
        let mut chars = self.chars();
        if self.col < chars.len() {
            chars.remove(self.col);
            self.lines[self.row] = chars.into_iter().collect();
            self.mark_changed();
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
            self.mark_changed();
        }
    }
    fn enter(&mut self) {
        let chars = self.chars();
        let before: String = chars[..self.col.min(chars.len())].iter().collect();
        let after: String = chars[self.col.min(chars.len())..].iter().collect();
        let indent: String = if self.autoindent {
            before
                .chars()
                .take_while(|ch| matches!(ch, ' ' | '\t'))
                .collect()
        } else {
            String::new()
        };
        self.lines[self.row] = before;
        self.row += 1;
        self.lines.insert(self.row, format!("{indent}{after}"));
        self.col = indent.chars().count();
        self.mark_changed();
    }
    fn delete_insert_prefix(&mut self, word: bool) {
        if self.col == 0 {
            return;
        }
        let mut chars = self.chars();
        let end = self.col.min(chars.len());
        let mut start = end;
        if word {
            while start > 0 && chars[start - 1].is_whitespace() {
                start -= 1;
            }
            if start > 0 {
                let class = |ch: char| ch.is_alphanumeric() || ch == '_';
                let in_word = class(chars[start - 1]);
                while start > 0
                    && !chars[start - 1].is_whitespace()
                    && class(chars[start - 1]) == in_word
                {
                    start -= 1;
                }
            }
        } else {
            start = 0;
        }
        chars.drain(start..end);
        self.lines[self.row] = chars.into_iter().collect();
        self.col = start;
        self.mark_changed();
    }
    fn insert_shift(&mut self, right: bool) {
        let mut chars = self.chars();
        let leading = chars
            .iter()
            .take_while(|ch| matches!(ch, ' ' | '\t'))
            .count();
        if right {
            chars.splice(0..0, std::iter::repeat_n(' ', self.shiftwidth));
            self.col += self.shiftwidth;
        } else if leading > 0 {
            let removed = if chars[0] == '\t' {
                1
            } else {
                leading.min(self.shiftwidth)
            };
            chars.drain(0..removed);
            self.col = self.col.saturating_sub(removed);
        } else {
            return;
        }
        self.lines[self.row] = chars.into_iter().collect();
        self.mark_changed();
    }
    fn substitute(&mut self, spec: SubstituteSpec) {
        if self.readonly && !spec.count_only {
            self.message = "Read-only buffer".into();
            return;
        }
        let regex = match vim_regex(
            &spec.pattern,
            spec.case_insensitive
                .unwrap_or_else(|| self.search_case_insensitive(&spec.pattern)),
        ) {
            Ok(regex) => regex,
            Err(e) => {
                self.message = e.to_string();
                return;
            }
        };
        let mut changes = 0;
        for row in spec.lines.clone() {
            changes += if spec.global {
                regex.find_iter(&self.lines[row]).count()
            } else {
                usize::from(regex.is_match(&self.lines[row]))
            };
        }
        if changes == 0 {
            if !spec.suppress_missing {
                self.message = "Pattern not found".into();
            }
            return;
        }
        if spec.count_only {
            self.message = format!("{changes} matches");
            return;
        }
        self.snapshot();
        let mut updated = Vec::with_capacity(self.lines.len());
        for (row, line) in self.lines.iter().enumerate() {
            let replaced = if spec.lines.contains(&row) {
                if spec.global {
                    regex
                        .replace_all(line, spec.replacement.as_str())
                        .into_owned()
                } else {
                    regex.replace(line, spec.replacement.as_str()).into_owned()
                }
            } else {
                line.clone()
            };
            updated.extend(replaced.split('\n').map(str::to_owned));
        }
        self.lines = updated;
        self.row = self.row.min(self.lines.len() - 1);
        self.clamp();
        self.mark_changed();
        self.message = format!("{changes} substitutions");
    }
    fn command(&mut self) {
        let command = self.prompt.trim().to_owned();
        if let Some(result) = parse_substitute(
            &command,
            self.row,
            self.lines.len(),
            self.search.as_ref().map(|(pattern, _)| pattern.as_str()),
        ) {
            match result {
                Ok(spec) => self.substitute(spec),
                Err(error) => self.message = error,
            }
            return;
        }
        if let Some(result) = parse_global(
            &command,
            self.row,
            self.lines.len(),
            self.search.as_ref().map(|(pattern, _)| pattern.as_str()),
        ) {
            match result {
                Ok(spec) => self.global_command(spec),
                Err(error) => self.message = error,
            }
            return;
        }
        if let Some(result) = parse_line_command(&command, self.row, self.lines.len()) {
            match result {
                Ok((lines, name, args)) => self.line_command(lines, name, args),
                Err(error) => self.message = error,
            }
            return;
        }
        match command.as_str() {
            "$undo" => self.undo(false),
            "$redo" => self.undo(true),
            "w" | "write" => {
                if let Err(e) = self.save() {
                    self.message = e.to_string()
                }
            }
            "q" | "quit" | "$quit" => {
                if self.modified {
                    self.message = "No write since last change (:q! to quit)".into()
                } else {
                    self.quit = true
                }
            }
            "q!" | "quit!" => self.quit = true,
            "wq" | "x" | "$end" => {
                if let Err(e) = self.save() {
                    self.message = e.to_string()
                } else {
                    self.quit = true
                }
            }
            "noh" | "nohlsearch" => {
                self.highlight = false;
                self.message.clear()
            }
            _ if command.starts_with("r ") || command.starts_with("read ") => {
                if self.readonly {
                    self.message = "Read-only buffer".into();
                } else {
                    let path = command.split_once(' ').unwrap().1.trim();
                    match fs::read_to_string(path) {
                        Ok(content) => {
                            let imported = content.lines().map(str::to_owned).collect::<Vec<_>>();
                            if !imported.is_empty() {
                                self.snapshot();
                                let at = self.row + 1;
                                self.row = at + imported.len() - 1;
                                self.lines.splice(at..at, imported);
                                self.col = 0;
                                self.mark_changed();
                            }
                        }
                        Err(error) => self.message = error.to_string(),
                    }
                }
            }
            _ if command.starts_with("grep ") => self.grep_project(command[5..].trim()),
            "GitStatus" => self.open_git_status(),
            "GitDiff" => self.open_git_diff(),
            _ if command.starts_with("e ") || command.starts_with("edit ") => {
                if self.modified {
                    self.message = "No write since last change (:w or :q!)".into()
                } else {
                    let name = command
                        .split_once(' ')
                        .map(|(_, name)| name.trim())
                        .unwrap_or("");
                    if name.is_empty() {
                        self.message = "File name required".into()
                    } else {
                        let path = PathBuf::from(name);
                        let root = self.project_root.clone();
                        match Self::open(Some(path), self.readonly) {
                            Ok(mut file) => {
                                file.project_root = root;
                                *self = file
                            }
                            Err(e) => self.message = e.to_string(),
                        }
                    }
                }
            }
            _ => self.message = format!("Not an editor command: {command}"),
        }
    }
    fn line_command(&mut self, lines: std::ops::Range<usize>, name: &str, args: &str) {
        if matches!(name, "undo" | "redo") {
            if !args.is_empty() || lines.start != self.row || lines.end != self.row + 1 {
                self.message = "Invalid undo/redo argument".into();
            } else if self.readonly {
                self.message = "Read-only buffer".into();
            } else {
                self.undo(name == "redo");
            }
            return;
        }
        if self.readonly && !matches!(name, "y" | "yank") {
            self.message = "Read-only buffer".into();
            return;
        }
        match name {
            "d" | "delete" | "y" | "yank" => {
                if !args.is_empty()
                    && (args.chars().count() != 1
                        || !args.chars().all(|ch| ch.is_ascii_alphanumeric()))
                {
                    self.message = format!("Invalid {name} argument");
                    return;
                }
                self.active_register = args.chars().next();
                self.register = self.lines[lines.clone()].to_vec();
                self.register_linewise = true;
                self.register_blockwise = false;
                self.store_register(matches!(name, "y" | "yank"));
                if matches!(name, "d" | "delete") {
                    self.snapshot();
                    self.lines.drain(lines.clone());
                    if self.lines.is_empty() {
                        self.lines.push(String::new());
                    }
                    self.row = lines.start.min(self.lines.len() - 1);
                    self.col = 0;
                    self.mark_changed();
                }
            }
            "pu" | "put" | "put!" => {
                if !args.is_empty()
                    && (args.chars().count() != 1
                        || !args.chars().all(|ch| ch.is_ascii_alphanumeric()))
                {
                    self.message = "Invalid put argument".into();
                    return;
                }
                let inserted_lines = if let Some(name) = args.chars().next() {
                    self.named_registers
                        .get(&name.to_ascii_lowercase())
                        .map(|register| register.lines.clone())
                        .unwrap_or_default()
                } else {
                    self.register.clone()
                };
                if inserted_lines.is_empty() {
                    self.message = "Register is empty".into();
                } else {
                    self.snapshot();
                    let at = if name.ends_with('!') {
                        lines.start
                    } else {
                        lines.end.min(self.lines.len())
                    };
                    let inserted = inserted_lines.len();
                    self.lines.splice(at..at, inserted_lines);
                    self.row = at + inserted - 1;
                    self.col = 0;
                    self.mark_changed();
                }
            }
            "sort" | "sort!" => {
                if args.chars().any(|flag| !matches!(flag, 'i' | 'u' | 'r')) {
                    self.message = "Unsupported sort flag".into();
                    return;
                }
                let case_insensitive = args.contains('i');
                let unique = args.contains('u');
                let reverse = name.ends_with('!') ^ args.contains('r');
                let mut sorted = self.lines[lines.clone()].to_vec();
                sorted.sort_by(|a, b| {
                    if case_insensitive {
                        a.to_lowercase().cmp(&b.to_lowercase())
                    } else {
                        a.cmp(b)
                    }
                });
                if unique {
                    sorted.dedup_by(|a, b| {
                        if case_insensitive {
                            a.to_lowercase() == b.to_lowercase()
                        } else {
                            a == b
                        }
                    });
                }
                if reverse {
                    sorted.reverse();
                }
                if self.lines[lines.clone()] != sorted {
                    self.snapshot();
                    self.lines.splice(lines.clone(), sorted);
                    self.row = lines.start.min(self.lines.len() - 1);
                    self.clamp();
                    self.mark_changed();
                }
            }
            "j" | "join" | "join!" => {
                if !args.is_empty() {
                    self.message = "Invalid join argument".into();
                    return;
                }
                let end = lines.end.max(lines.start + 2).min(self.lines.len());
                if end > lines.start + 1 {
                    self.snapshot();
                    let separator = if name.ends_with('!') { "" } else { " " };
                    let first = self.lines[lines.start].trim_end();
                    let joined = std::iter::once(first)
                        .chain(
                            self.lines[lines.start + 1..end]
                                .iter()
                                .map(|line| line.trim()),
                        )
                        .collect::<Vec<_>>()
                        .join(separator);
                    self.lines.splice(lines.start..end, [joined]);
                    self.row = lines.start;
                    self.col = 0;
                    self.mark_changed();
                }
            }
            "m" | "move" | "t" | "copy" => {
                let Some((destination, remaining)) = ex_address(args, self.row, self.lines.len())
                else {
                    self.message = "Destination line required".into();
                    return;
                };
                if destination > self.lines.len() || !remaining.trim().is_empty() {
                    self.message = "Invalid destination line".into();
                    return;
                }
                let moving = matches!(name, "m" | "move");
                if moving && destination > lines.start && destination <= lines.end {
                    self.message = "Destination inside moved range".into();
                    return;
                }
                let copied = self.lines[lines.clone()].to_vec();
                self.snapshot();
                let at = if moving {
                    self.lines.drain(lines.clone());
                    if destination > lines.end {
                        destination - copied.len()
                    } else {
                        destination
                    }
                } else {
                    destination
                };
                self.lines.splice(at..at, copied.clone());
                self.row = at + copied.len() - 1;
                self.col = 0;
                self.mark_changed();
            }
            _ => unreachable!(),
        }
    }
    fn global_command(&mut self, spec: GlobalSpec) {
        let regex = match vim_regex(&spec.pattern, self.search_case_insensitive(&spec.pattern)) {
            Ok(regex) => regex,
            Err(error) => {
                self.message = error.to_string();
                return;
            }
        };
        let selected = spec
            .lines
            .filter(|&row| regex.is_match(&self.lines[row]) != spec.inverse)
            .collect::<Vec<_>>();
        if selected.is_empty() {
            self.message = "Pattern not found".into();
            return;
        }
        if let Some(result) = parse_substitute(
            &spec.operation,
            selected[0],
            self.lines.len(),
            self.search.as_ref().map(|(pattern, _)| pattern.as_str()),
        ) {
            match result {
                Ok(mut substitution) => {
                    substitution.lines = selected;
                    self.substitute(substitution);
                }
                Err(error) => self.message = error,
            }
            return;
        }
        match spec.operation.as_str() {
            "d" | "delete" | "y" | "yank" => {
                if self.readonly && matches!(spec.operation.as_str(), "d" | "delete") {
                    self.message = "Read-only buffer".into();
                    return;
                }
                self.register = selected
                    .iter()
                    .map(|&row| self.lines[row].clone())
                    .collect();
                self.register_linewise = true;
                self.register_blockwise = false;
                self.store_register(matches!(spec.operation.as_str(), "y" | "yank"));
                if matches!(spec.operation.as_str(), "d" | "delete") {
                    self.snapshot();
                    for row in selected.iter().rev() {
                        self.lines.remove(*row);
                    }
                    if self.lines.is_empty() {
                        self.lines.push(String::new());
                    }
                    self.row = selected[0].min(self.lines.len() - 1);
                    self.col = 0;
                    self.mark_changed();
                }
                self.message = format!("{} lines", selected.len());
            }
            "" | "p" | "print" => {
                self.message = format!(
                    "{} matches: {}",
                    selected.len(),
                    selected
                        .iter()
                        .take(3)
                        .map(|&row| format!("{}:{}", row + 1, self.lines[row]))
                        .collect::<Vec<_>>()
                        .join(" | ")
                );
            }
            _ => self.message = format!("Unsupported global command: {}", spec.operation),
        }
    }
    fn mark_changed(&mut self) {
        if let Some(position) = self.change_candidate.take() {
            if self.change_list.last() != Some(&position) {
                self.change_list.push(position);
                if self.change_list.len() > 100 {
                    self.change_list.remove(0);
                }
            }
            self.change_index = self.change_list.len();
        }
        self.modified = true;
        self.revision = self.revision.wrapping_add(1)
    }
    fn snapshot(&mut self) {
        self.change_candidate = Some((self.row, self.col));
        let bytes = self.lines.iter().map(String::len).sum::<usize>();
        self.undo.push((
            self.lines.clone(),
            self.row,
            self.col,
            self.trailing_newline,
        ));
        self.undo_bytes = self.undo_bytes.saturating_add(bytes);
        self.redo.clear();
        while self.undo.len() > 1 && (self.undo.len() > 500 || self.undo_bytes > 32 * 1024 * 1024) {
            let removed = self.undo.remove(0);
            self.undo_bytes = self
                .undo_bytes
                .saturating_sub(removed.0.iter().map(String::len).sum::<usize>());
        }
    }
    fn undo(&mut self, redo: bool) {
        let previous = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some((lines, row, col, trailing_newline)) = previous {
            let previous_bytes = lines.iter().map(String::len).sum::<usize>();
            let current = std::mem::replace(&mut self.lines, lines);
            let current_bytes = current.iter().map(String::len).sum::<usize>();
            let state = (current, self.row, self.col, self.trailing_newline);
            if redo {
                self.undo.push(state);
                self.undo_bytes = self.undo_bytes.saturating_add(current_bytes);
                while self.undo.len() > 1
                    && (self.undo.len() > 500 || self.undo_bytes > 32 * 1024 * 1024)
                {
                    let removed = self.undo.remove(0);
                    self.undo_bytes = self
                        .undo_bytes
                        .saturating_sub(removed.0.iter().map(String::len).sum::<usize>());
                }
            } else {
                self.redo.push(state);
                self.undo_bytes = self.undo_bytes.saturating_sub(previous_bytes);
            }
            self.row = row;
            self.col = col;
            self.trailing_newline = trailing_newline;
            let content = self.content();
            self.revision = self.revision.wrapping_add(1);
            self.modified = self
                .disk_hash
                .map_or(self.lines != [String::new()], |saved| {
                    hash_bytes(content.as_bytes()) != saved
                });
        }
    }
    fn step(&mut self, motion: Motion) {
        match motion {
            Motion::Left => self.move_horizontal(false),
            Motion::Right => self.move_horizontal(true),
            Motion::Up => self.move_vertical(false),
            Motion::Down => self.move_vertical(true),
            Motion::Start => self.col = 0,
            Motion::FirstNonBlank => {
                self.col = self
                    .chars()
                    .iter()
                    .take_while(|c| c.is_whitespace())
                    .count()
            }
            Motion::End => self.col = self.chars().len().saturating_sub(1),
            Motion::FirstLine => {
                self.row = 0;
                self.clamp()
            }
            Motion::LastLine => {
                self.row = self.lines.len() - 1;
                self.clamp()
            }
            Motion::Find {
                target,
                forward,
                till,
                repeat,
            } => self.step_find(target, forward, till, repeat),
            Motion::MatchBracket => self.step_match_bracket(),
            Motion::ParagraphBackward => {
                self.row = (0..self.row)
                    .rev()
                    .find(|&row| self.lines[row].is_empty())
                    .unwrap_or(0);
                self.col = 0;
            }
            Motion::ParagraphForward => {
                self.row = (self.row + 1..self.lines.len())
                    .find(|&row| self.lines[row].is_empty())
                    .unwrap_or(self.lines.len() - 1);
                self.col = 0;
            }
            Motion::ScreenTop | Motion::ScreenMiddle | Motion::ScreenBottom => {
                self.step_count(motion, 1);
            }
            Motion::Word
            | Motion::BigWord
            | Motion::BackWord
            | Motion::BigBackWord
            | Motion::WordEnd
            | Motion::BigWordEnd => self.step_word(motion),
        }
    }
    fn step_count(&mut self, motion: Motion, count: usize) {
        let target = match motion {
            Motion::ScreenTop => Some(self.viewport_top.saturating_add(count.saturating_sub(1))),
            Motion::ScreenMiddle => Some(self.viewport_top + self.viewport_height / 2),
            Motion::ScreenBottom => Some(
                self.viewport_top
                    .saturating_add(self.viewport_height)
                    .saturating_sub(count),
            ),
            _ => None,
        };
        if let Some(target) = target {
            self.row = target.min(self.lines.len() - 1);
            self.col = self.lines[self.row]
                .chars()
                .take_while(|ch| ch.is_whitespace())
                .count();
            self.clamp();
        } else {
            for index in 0..count {
                self.step(if index == 0 {
                    motion
                } else {
                    motion.repeated()
                });
            }
        }
    }
    fn step_find(&mut self, target: char, forward: bool, till: bool, repeat: bool) {
        let chars = self.chars();
        let match_col = if forward {
            let from = self.col.saturating_add(1 + usize::from(till && repeat));
            (from..chars.len()).find(|&i| chars[i] == target)
        } else {
            let before = self.col.saturating_sub(usize::from(till && repeat));
            (0..before).rev().find(|&i| chars[i] == target)
        };
        if let Some(found) = match_col {
            self.col = if till {
                if forward {
                    found.saturating_sub(1)
                } else {
                    found + 1
                }
            } else {
                found
            };
        }
    }
    fn step_match_bracket(&mut self) {
        let current = self.chars();
        let Some((column, bracket)) = current
            .iter()
            .enumerate()
            .skip(self.col)
            .find(|(_, ch)| matches!(**ch, '(' | ')' | '[' | ']' | '{' | '}'))
        else {
            return;
        };
        let (open, close, forward) = match *bracket {
            '(' => ('(', ')', true),
            '[' => ('[', ']', true),
            '{' => ('{', '}', true),
            ')' => ('(', ')', false),
            ']' => ('[', ']', false),
            '}' => ('{', '}', false),
            _ => unreachable!(),
        };
        let source: Vec<char> = self.lines.join("\n").chars().collect();
        let origin = self.lines[..self.row]
            .iter()
            .map(|line| line.chars().count() + 1)
            .sum::<usize>()
            + column;
        let mut depth = 1usize;
        let mut match_index = None;
        if forward {
            for (index, ch) in source.iter().enumerate().skip(origin + 1) {
                if *ch == open {
                    depth += 1;
                }
                if *ch == close {
                    depth -= 1;
                }
                if depth == 0 {
                    match_index = Some(index);
                    break;
                }
            }
        } else {
            for index in (0..origin).rev() {
                if source[index] == close {
                    depth += 1;
                }
                if source[index] == open {
                    depth -= 1;
                }
                if depth == 0 {
                    match_index = Some(index);
                    break;
                }
            }
        }
        if let Some(index) = match_index {
            let before = &source[..index];
            self.row = before.iter().filter(|&&ch| ch == '\n').count();
            self.col = before.iter().rev().take_while(|&&ch| ch != '\n').count();
        }
    }
    fn step_word(&mut self, motion: Motion) {
        let big = matches!(
            motion,
            Motion::BigWord | Motion::BigBackWord | Motion::BigWordEnd
        );
        let backward = matches!(motion, Motion::BackWord | Motion::BigBackWord);
        let end = matches!(motion, Motion::WordEnd | Motion::BigWordEnd);
        let text: Vec<char> = self.lines.join("\n").chars().collect();
        if text.is_empty() {
            return;
        }
        let origin = self.lines[..self.row]
            .iter()
            .map(|line| line.chars().count() + 1)
            .sum::<usize>()
            + self.col;
        let classify = |c: char| -> u8 {
            if c.is_whitespace() {
                0
            } else if big || c.is_alphanumeric() || c == '_' {
                1
            } else {
                2
            }
        };
        let mut index = origin.min(text.len() - 1);
        if backward {
            if index == 0 {
                return;
            }
            index -= 1;
            while index > 0 && classify(text[index]) == 0 {
                index -= 1;
            }
            let group = classify(text[index]);
            while index > 0 && classify(text[index - 1]) == group {
                index -= 1;
            }
        } else if end {
            if index + 1 >= text.len() {
                return;
            }
            index += 1;
            while index < text.len() && classify(text[index]) == 0 {
                index += 1;
            }
            if index >= text.len() {
                return;
            }
            let group = classify(text[index]);
            while index + 1 < text.len() && classify(text[index + 1]) == group {
                index += 1;
            }
        } else {
            let group = classify(text[index]);
            index += 1;
            if group != 0 {
                while index < text.len() && classify(text[index]) == group {
                    index += 1;
                }
            }
            while index < text.len() && classify(text[index]) == 0 {
                index += 1;
            }
            if index >= text.len() {
                return;
            }
        }
        let mut row = 0;
        let mut col = index;
        while row + 1 < self.lines.len() {
            let length = self.lines[row].chars().count();
            if col <= length {
                break;
            }
            col -= length + 1;
            row += 1;
        }
        self.row = row;
        self.col = col.min(self.chars().len().saturating_sub(1));
    }
    fn store_register(&mut self, yank: bool) {
        let selected = self.active_register.take();
        if selected == Some('_') {
            if let Some(old) = self.named_registers.get(&'"') {
                self.register = old.lines.clone();
                self.register_linewise = old.linewise;
                self.register_blockwise = old.blockwise;
            } else {
                self.register.clear();
            }
            return;
        }
        let new = StoredRegister {
            lines: self.register.clone(),
            linewise: self.register_linewise,
            blockwise: self.register_blockwise,
        };
        self.named_registers.insert('"', new.clone());
        if yank {
            self.named_registers.insert('0', new.clone());
        } else if new.linewise || new.lines.len() > 1 {
            for number in (2..=9).rev() {
                if let Some(old) = self
                    .named_registers
                    .get(&char::from_digit(number - 1, 10).unwrap())
                    .cloned()
                {
                    self.named_registers
                        .insert(char::from_digit(number, 10).unwrap(), old);
                }
            }
            self.named_registers.insert('1', new.clone());
        } else {
            self.named_registers.insert('-', new.clone());
        }
        let Some(name) = selected else { return };
        if name == '"' {
            return;
        }
        if matches!(name, '+' | '*') {
            if let Err(error) = clipboard::copy(&new.text()) {
                self.message = format!("Clipboard: {error}");
            }
            self.named_registers.insert(name, new);
            return;
        }
        let key = name.to_ascii_lowercase();
        if name.is_ascii_uppercase() {
            if let Some(old) = self.named_registers.get_mut(&key) {
                if old.linewise && new.linewise {
                    old.lines.extend(new.lines);
                    return;
                }
                if !old.linewise && !new.linewise && !old.blockwise && !new.blockwise {
                    if let (Some(last), Some(first)) = (old.lines.last_mut(), new.lines.first()) {
                        last.push_str(first);
                        old.lines.extend(new.lines.into_iter().skip(1));
                        return;
                    }
                }
            }
        }
        self.named_registers.insert(key, new);
    }
    fn indent_rows(&mut self, first: usize, last: usize, right: bool) {
        if self.readonly {
            self.message = "Read-only buffer".into();
            return;
        }
        let last = last.min(self.lines.len() - 1);
        let changed = (first..=last).any(|row| {
            let line = &self.lines[row];
            if right {
                !line.is_empty()
            } else {
                line.starts_with(' ') || line.starts_with('\t')
            }
        });
        if !changed {
            return;
        }
        self.snapshot();
        for row in first..=last {
            let line = &mut self.lines[row];
            if right {
                if !line.is_empty() {
                    line.insert_str(0, &" ".repeat(self.shiftwidth));
                }
            } else if line.starts_with('\t') {
                line.remove(0);
            } else {
                let spaces = line
                    .bytes()
                    .take_while(|&byte| byte == b' ')
                    .take(self.shiftwidth)
                    .count();
                line.drain(..spaces);
            }
        }
        self.row = first;
        self.col = self.lines[first]
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .count();
        self.clamp();
        self.mark_changed();
    }
    fn shift_motion(&mut self, right: bool, motion: Motion, count: usize) {
        let previous = (self.row, self.col);
        self.step_count(motion, count);
        let target = (self.row, self.col);
        (self.row, self.col) = previous;
        let mut last = previous.0.max(target.0);
        if target.0 > previous.0
            && target.1 == 0
            && matches!(
                motion,
                Motion::Word | Motion::BigWord | Motion::ParagraphForward
            )
        {
            last = last.saturating_sub(1);
        }
        self.indent_rows(previous.0.min(target.0), last, right);
    }
    fn operate_lines(&mut self, op: Operator, count: usize) {
        let end = (self.row + count).min(self.lines.len());
        self.register = self.lines[self.row..end].to_vec();
        self.register_linewise = true;
        self.register_blockwise = false;
        self.store_register(op == Operator::Yank);
        if op == Operator::Yank {
            return;
        }
        self.snapshot();
        self.lines.drain(self.row..end);
        if self.lines.is_empty() {
            self.lines.push(String::new())
        }
        self.row = self.row.min(self.lines.len() - 1);
        self.col = 0;
        self.mark_changed();
        if op == Operator::Change {
            self.mode = Mode::Insert
        }
    }
    fn operate_motion(&mut self, op: Operator, motion: Motion, count: usize) {
        let start = (self.row, self.col);
        self.step_count(motion, count);
        let mut end = (self.row, self.col);
        if matches!(motion, Motion::End | Motion::WordEnd | Motion::BigWordEnd) && end >= start {
            end.1 = (end.1 + 1).min(self.chars().len());
        }
        if start.0 != end.0 {
            let (first, mut last) = if start < end {
                (start, end)
            } else {
                (end, start)
            };
            if matches!(motion, Motion::MatchBracket) {
                last.1 = (last.1 + 1).min(self.lines[last.0].chars().count());
            }
            let mut selected = vec![self.lines[first.0].chars().skip(first.1).collect()];
            selected.extend(self.lines[first.0 + 1..last.0].iter().cloned());
            selected.push(self.lines[last.0].chars().take(last.1).collect());
            self.register = selected;
            self.register_linewise = false;
            self.register_blockwise = false;
            self.store_register(op == Operator::Yank);
            if op != Operator::Yank {
                (self.row, self.col) = start;
                self.snapshot();
                let prefix: String = self.lines[first.0].chars().take(first.1).collect();
                let suffix: String = self.lines[last.0].chars().skip(last.1).collect();
                self.lines
                    .splice(first.0..=last.0, [format!("{prefix}{suffix}")]);
                self.mark_changed();
                if op == Operator::Change {
                    self.mode = Mode::Insert
                }
            }
            self.row = first.0;
            self.col = first.1.min(self.chars().len().saturating_sub(1));
            return;
        }
        let chars = self.chars();
        let (a, b) = if start.1 <= end.1 {
            (start.1, end.1)
        } else {
            (end.1, start.1)
        };
        let b = if matches!(motion, Motion::Find { .. } | Motion::MatchBracket) && a != b {
            (b + 1).min(chars.len())
        } else {
            b
        };
        if a == b {
            return;
        }
        self.register = vec![chars[a..b].iter().collect()];
        self.register_linewise = false;
        self.register_blockwise = false;
        self.store_register(op == Operator::Yank);
        if op != Operator::Yank {
            (self.row, self.col) = start;
            self.snapshot();
            self.lines[self.row] = chars[..a].iter().chain(chars[b..].iter()).collect();
            self.col = a.min(self.chars().len().saturating_sub(1));
            self.mark_changed();
            if op == Operator::Change {
                self.mode = Mode::Insert
            }
        } else {
            (self.row, self.col) = start
        }
    }
    fn paragraph_range(&self, inner: bool, count: usize) -> (usize, usize) {
        let mut blocks = Vec::new();
        let mut first = 0;
        while first < self.lines.len() {
            let blank = self.lines[first].is_empty();
            let mut last = first;
            while last + 1 < self.lines.len() && self.lines[last + 1].is_empty() == blank {
                last += 1;
            }
            blocks.push((first, last, blank));
            first = last + 1;
        }
        let index = blocks
            .iter()
            .position(|(a, b, _)| *a <= self.row && self.row <= *b)
            .unwrap_or(0);
        if inner {
            return (
                blocks[index].0,
                blocks[(index + count.saturating_sub(1)).min(blocks.len() - 1)].1,
            );
        }
        let mut first_index = index;
        let mut last_index = index;
        let mut cursor = index;
        for _ in 0..count {
            if blocks[cursor].2 && cursor + 1 < blocks.len() {
                cursor += 1;
            }
            if blocks[cursor].2 {
                first_index = first_index.min(cursor.saturating_sub(1));
                last_index = last_index.max(cursor);
                break;
            }
            last_index = cursor;
            if cursor + 1 < blocks.len() && blocks[cursor + 1].2 {
                last_index = cursor + 1;
                cursor += 2;
            } else {
                if first_index == index && index > 0 && blocks[index - 1].2 {
                    first_index = index - 1;
                }
                cursor += 1;
            }
            if cursor >= blocks.len() {
                break;
            }
        }
        (blocks[first_index].0, blocks[last_index].1)
    }
    fn delimited_range(&self, object: char, count: usize) -> Option<(usize, usize)> {
        let (open, close) = match object {
            '(' | ')' => ('(', ')'),
            '[' | ']' => ('[', ']'),
            '{' | '}' => ('{', '}'),
            _ => return None,
        };
        let cursor = self.lines[..self.row]
            .iter()
            .map(|line| line.chars().count() + 1)
            .sum::<usize>()
            + self.col;
        let mut stack = Vec::new();
        let mut surrounding = Vec::new();
        for (index, ch) in self.lines.join("\n").chars().enumerate() {
            if ch == open {
                stack.push(index);
            } else if ch == close {
                if let Some(first) = stack.pop() {
                    if first <= cursor && cursor <= index {
                        surrounding.push((first, index));
                    }
                }
            }
        }
        surrounding.sort_unstable_by_key(|(first, _)| std::cmp::Reverse(*first));
        surrounding.get(count.saturating_sub(1)).copied()
    }
    fn operate_delimited_object(&mut self, op: Operator, inner: bool, object: char, count: usize) {
        let Some((open, close)) = self.delimited_range(object, count) else {
            self.message = format!("Text object {object} not found");
            return;
        };
        let (start, end) = if inner {
            (open + 1, close)
        } else {
            (open, close + 1)
        };
        let mut chars: Vec<char> = self.lines.join("\n").chars().collect();
        self.register = chars[start..end]
            .iter()
            .collect::<String>()
            .split('\n')
            .map(str::to_owned)
            .collect();
        self.register_linewise = false;
        self.register_blockwise = false;
        self.store_register(op == Operator::Yank);
        if op == Operator::Yank {
            return;
        }
        if start < end {
            self.snapshot();
            let before = &chars[..start];
            let row = before.iter().filter(|&&ch| ch == '\n').count();
            let col = before.iter().rev().take_while(|&&ch| ch != '\n').count();
            chars.drain(start..end);
            self.lines = chars
                .iter()
                .collect::<String>()
                .split('\n')
                .map(str::to_owned)
                .collect();
            self.row = row.min(self.lines.len() - 1);
            self.col = col.min(self.lines[self.row].chars().count().saturating_sub(1));
            self.mark_changed();
        }
        if op == Operator::Change {
            self.mode = Mode::Insert;
        }
    }
    fn operate_text_object(&mut self, op: Operator, inner: bool, object: char, count: usize) {
        if object == 'p' {
            let (first, last) = self.paragraph_range(inner, count);
            let previous = (self.row, self.col);
            self.row = first;
            self.operate_lines(op, last - first + 1);
            if op == Operator::Yank {
                (self.row, self.col) = previous;
            }
            return;
        }
        if matches!(object, '(' | ')' | '[' | ']' | '{' | '}') {
            self.operate_delimited_object(op, inner, object, count);
            return;
        }
        let chars = self.chars();
        if chars.is_empty() {
            return;
        }
        let cursor = self.col.min(chars.len() - 1);
        let selected = if object == 'w' || object == 'W' {
            let big = object == 'W';
            let class = |c: char| -> u8 {
                if c.is_whitespace() {
                    0
                } else if big || c.is_alphanumeric() || c == '_' {
                    1
                } else {
                    2
                }
            };
            let mut start = cursor;
            if class(chars[start]) == 0 {
                while start < chars.len() && class(chars[start]) == 0 {
                    start += 1;
                }
            }
            if start >= chars.len() {
                None
            } else {
                let group = class(chars[start]);
                while start > 0 && class(chars[start - 1]) == group {
                    start -= 1;
                }
                let mut end = start;
                let mut trailing_space = false;
                for _ in 0..count {
                    if end >= chars.len() {
                        break;
                    }
                    let group = class(chars[end]);
                    while end < chars.len() && class(chars[end]) == group {
                        end += 1;
                    }
                    let before_space = end;
                    while end < chars.len() && class(chars[end]) == 0 {
                        end += 1;
                    }
                    trailing_space = end > before_space;
                }
                if inner {
                    while end > start && class(chars[end - 1]) == 0 {
                        end -= 1;
                    }
                } else if !trailing_space {
                    while start > 0 && class(chars[start - 1]) == 0 {
                        start -= 1;
                    }
                }
                Some((start, end))
            }
        } else if matches!(object, '"' | '\'' | '`') {
            let mut open = None;
            let mut pair = None;
            for (index, ch) in chars.iter().enumerate() {
                if *ch == object && (index == 0 || chars[index - 1] != '\\') {
                    if let Some(left) = open.take() {
                        if left <= cursor && cursor <= index {
                            pair = Some((left, index));
                            break;
                        }
                    } else {
                        open = Some(index);
                    }
                }
            }
            pair.map(|(left, right)| {
                if inner {
                    (left + 1, right)
                } else {
                    (left, right + 1)
                }
            })
        } else {
            return;
        };
        let Some((start, end)) = selected else {
            self.message = format!("Text object {object} not found");
            return;
        };
        self.register = vec![chars[start..end].iter().collect()];
        self.register_linewise = false;
        self.register_blockwise = false;
        self.store_register(op == Operator::Yank);
        if op != Operator::Yank {
            self.snapshot();
            self.lines[self.row] = chars[..start].iter().chain(chars[end..].iter()).collect();
            self.col = start.min(self.chars().len().saturating_sub(1));
            self.mark_changed();
            if op == Operator::Change {
                self.mode = Mode::Insert;
            }
        }
    }
    fn repeatable(action: Action) -> bool {
        matches!(
            action,
            Action::Insert
                | Action::Append
                | Action::InsertFirstNonBlank
                | Action::AppendEnd
                | Action::OpenBelow
                | Action::OpenAbove
                | Action::DeleteChar(_)
                | Action::DeleteBefore(_)
                | Action::Substitute(_)
                | Action::JoinLines(_)
                | Action::ToggleCase(_)
                | Action::Indent(_, _)
                | Action::ShiftMotion(_, _, _)
                | Action::ShiftTextObject(_, _, _, _)
                | Action::Put(_, _)
                | Action::Replace(_, _)
        ) || matches!(action, Action::Operate(op, _, _) | Action::Lines(op, _)
                | Action::TextObject(op, _, _, _) if op != Operator::Yank)
    }
    fn repeat_last(&mut self, count: usize) {
        let Some(change) = self.last_change.clone() else {
            self.message = "No change to repeat".into();
            return;
        };
        self.replaying_change = true;
        for _ in 0..count {
            for key in change.iter().copied() {
                self.handle(key);
            }
        }
        self.replaying_change = false;
    }
    fn play_macro(&mut self, name: char, count: usize) {
        let name = if name == '@' {
            let Some(previous) = self.last_played_macro else {
                self.message = "No previous macro".into();
                return;
            };
            previous
        } else {
            name.to_ascii_lowercase()
        };
        let Some(keys) = self.macros.get(&name).cloned() else {
            self.message = format!("Macro @{name} is empty");
            return;
        };
        if self.macro_depth >= 16 {
            self.message = "Macro recursion limit reached".into();
            return;
        }
        self.last_played_macro = Some(name);
        self.macro_depth += 1;
        for _ in 0..count {
            for key in keys.iter().copied() {
                self.handle(key);
            }
        }
        self.macro_depth -= 1;
    }
    fn apply(&mut self, action: Action) {
        let modifies = matches!(
            action,
            Action::Insert
                | Action::InsertFirstNonBlank
                | Action::Append
                | Action::AppendEnd
                | Action::OpenBelow
                | Action::OpenAbove
                | Action::DeleteChar(_)
                | Action::DeleteBefore(_)
                | Action::Substitute(_)
                | Action::JoinLines(_)
                | Action::ToggleCase(_)
                | Action::Indent(_, _)
                | Action::ShiftMotion(_, _, _)
                | Action::ShiftTextObject(_, _, _, _)
                | Action::Put(_, _)
                | Action::Replace(_, _)
        ) || matches!(action, Action::Operate(op, _, _) | Action::Lines(op, _) | Action::TextObject(op, _, _, _) if op != Operator::Yank);
        if modifies && self.readonly {
            self.message = "Read-only buffer".into();
            return;
        }
        match action {
            Action::Move(motion, count) => {
                let previous = (self.row, self.col);
                self.step_count(motion, count);
                if matches!(motion, Motion::FirstLine | Motion::LastLine) {
                    let target = (self.row, self.col);
                    (self.row, self.col) = previous;
                    self.record_jump(target);
                    (self.row, self.col) = target;
                }
            }
            Action::Definition => self.request_definition = true,
            Action::References => self.request_references = true,
            Action::Hover => self.request_hover = true,
            Action::ChangeList(forward, count) => self.jump_changes(forward, count),
            Action::GoLine(line) => {
                let previous = (self.row, self.col);
                self.row = line.map_or(self.lines.len() - 1, |n| {
                    n.saturating_sub(1).min(self.lines.len() - 1)
                });
                self.clamp();
                let target = (self.row, self.col);
                (self.row, self.col) = previous;
                self.record_jump(target);
                (self.row, self.col) = target;
            }
            Action::Operate(op, motion, count) => self.operate_motion(op, motion, count),
            Action::TextObject(op, inner, object, count) => {
                self.operate_text_object(op, inner, object, count)
            }
            Action::Lines(op, count) => self.operate_lines(op, count),
            Action::Insert => {
                self.snapshot();
                self.mode = Mode::Insert
            }
            Action::Append => {
                self.snapshot();
                self.col = (self.col + 1).min(self.chars().len());
                self.mode = Mode::Insert
            }
            Action::InsertFirstNonBlank => {
                self.snapshot();
                self.step(Motion::FirstNonBlank);
                self.mode = Mode::Insert
            }
            Action::AppendEnd => {
                self.snapshot();
                self.col = self.chars().len();
                self.mode = Mode::Insert
            }
            Action::OpenBelow => {
                self.snapshot();
                let indent = if self.autoindent {
                    self.lines[self.row]
                        .chars()
                        .take_while(|ch| matches!(ch, ' ' | '\t'))
                        .collect::<String>()
                } else {
                    String::new()
                };
                self.row += 1;
                self.col = indent.chars().count();
                self.lines.insert(self.row, indent);
                self.mark_changed();
                self.mode = Mode::Insert
            }
            Action::OpenAbove => {
                self.snapshot();
                let indent = if self.autoindent {
                    self.lines[self.row]
                        .chars()
                        .take_while(|ch| matches!(ch, ' ' | '\t'))
                        .collect::<String>()
                } else {
                    String::new()
                };
                self.col = indent.chars().count();
                self.lines.insert(self.row, indent);
                self.mark_changed();
                self.mode = Mode::Insert
            }
            Action::DeleteChar(count) => {
                let mut chars = self.chars();
                let end = (self.col + count).min(chars.len());
                if end == self.col {
                    return;
                }
                self.register = vec![chars[self.col..end].iter().collect()];
                self.register_linewise = false;
                self.register_blockwise = false;
                self.store_register(false);
                self.snapshot();
                for _ in 0..count {
                    if self.col < chars.len() {
                        chars.remove(self.col);
                    }
                }
                self.lines[self.row] = chars.into_iter().collect();
                self.clamp();
                self.mark_changed()
            }
            Action::DeleteBefore(count) => {
                let mut chars = self.chars();
                let start = self.col.saturating_sub(count);
                if start < self.col {
                    self.register = vec![chars[start..self.col].iter().collect()];
                    self.register_linewise = false;
                    self.register_blockwise = false;
                    self.store_register(false);
                    self.snapshot();
                    chars.drain(start..self.col);
                    self.lines[self.row] = chars.into_iter().collect();
                    self.col = start;
                    self.mark_changed()
                }
            }
            Action::Substitute(count) => {
                let mut chars = self.chars();
                let end = (self.col + count).min(chars.len());
                self.register = vec![chars[self.col..end].iter().collect()];
                self.register_linewise = false;
                self.register_blockwise = false;
                self.store_register(false);
                self.snapshot();
                chars.drain(self.col..end);
                self.lines[self.row] = chars.into_iter().collect();
                self.mark_changed();
                self.mode = Mode::Insert
            }
            Action::JoinLines(count) => {
                let end = (self.row + count.max(2)).min(self.lines.len());
                if end > self.row + 1 {
                    self.snapshot();
                    let first_length = self.lines[self.row].trim_end().chars().count();
                    let first = self.lines[self.row].trim_end();
                    let joined = std::iter::once(first)
                        .chain(self.lines[self.row + 1..end].iter().map(|line| line.trim()))
                        .collect::<Vec<_>>()
                        .join(" ");
                    self.lines.splice(self.row..end, [joined]);
                    self.col = first_length.min(self.chars().len().saturating_sub(1));
                    self.mark_changed();
                }
            }
            Action::ToggleCase(count) => {
                let chars = self.chars();
                if self.col < chars.len() {
                    self.snapshot();
                    let mut changed = chars;
                    for index in self.col..(self.col + count).min(changed.len()) {
                        let ch = changed[index];
                        changed[index] = if ch.is_lowercase() {
                            ch.to_uppercase().next().unwrap_or(ch)
                        } else {
                            ch.to_lowercase().next().unwrap_or(ch)
                        };
                    }
                    self.lines[self.row] = changed.into_iter().collect();
                    self.col = (self.col + count).min(self.chars().len().saturating_sub(1));
                    self.mark_changed();
                }
            }
            Action::Indent(right, count) => {
                let last = self.row.saturating_add(count.saturating_sub(1));
                self.indent_rows(self.row, last, right);
            }
            Action::ShiftMotion(right, motion, count) => self.shift_motion(right, motion, count),
            Action::ShiftTextObject(right, inner, object, count) => {
                let (first, last) = if object == 'p' {
                    self.paragraph_range(inner, count)
                } else if matches!(object, '(' | ')' | '[' | ']' | '{' | '}') {
                    let Some((open, close)) = self.delimited_range(object, count) else {
                        self.message = format!("Text object {object} not found");
                        return;
                    };
                    let chars: Vec<char> = self.lines.join("\n").chars().collect();
                    let (start, end) = if inner {
                        let content = &chars[open + 1..close];
                        let Some(first_content) = content.iter().position(|ch| !ch.is_whitespace())
                        else {
                            return;
                        };
                        let last_content =
                            content.iter().rposition(|ch| !ch.is_whitespace()).unwrap();
                        (open + 1 + first_content, open + 1 + last_content)
                    } else {
                        (open, close)
                    };
                    (
                        chars[..start].iter().filter(|&&ch| ch == '\n').count(),
                        chars[..end].iter().filter(|&&ch| ch == '\n').count(),
                    )
                } else {
                    (self.row, self.row)
                };
                self.indent_rows(first, last, right);
            }
            Action::Put(after, count) => {
                let selected = self.active_register.take();
                let source = match selected {
                    Some('"') | None => StoredRegister {
                        lines: self.register.clone(),
                        linewise: self.register_linewise,
                        blockwise: self.register_blockwise,
                    },
                    Some('+' | '*') => match clipboard::paste() {
                        Ok(text) => StoredRegister::from_text(&text),
                        Err(error) => {
                            self.message = format!("Clipboard: {error}");
                            return;
                        }
                    },
                    Some(name) => self
                        .named_registers
                        .get(&name.to_ascii_lowercase())
                        .cloned()
                        .unwrap_or(StoredRegister {
                            lines: Vec::new(),
                            linewise: false,
                            blockwise: false,
                        }),
                };
                if source.lines.is_empty() {
                    self.message = "Register is empty".into();
                    return;
                }
                self.snapshot();
                if source.linewise {
                    let mut at = self.row + usize::from(after);
                    for _ in 0..count {
                        for line in source.lines.clone() {
                            self.lines.insert(at, line);
                            at += 1
                        }
                    }
                    self.row = at.saturating_sub(1);
                    self.col = 0;
                } else if source.blockwise {
                    let at = self.col + usize::from(after);
                    for (offset, fragment) in source.lines.iter().enumerate() {
                        let row = self.row + offset;
                        if row >= self.lines.len() {
                            self.lines.push(String::new())
                        }
                        let mut chars: Vec<char> = self.lines[row].chars().collect();
                        if chars.len() < at {
                            chars.resize(at, ' ')
                        }
                        chars.splice(at..at, fragment.repeat(count).chars());
                        self.lines[row] = chars.into_iter().collect();
                    }
                    self.col = at;
                } else {
                    let text = source.lines.join("\n").repeat(count);
                    let chars = self.chars();
                    let at = (self.col + usize::from(after)).min(chars.len());
                    let prefix: String = chars[..at].iter().collect();
                    let suffix: String = chars[at..].iter().collect();
                    let joined = format!("{prefix}{text}{suffix}");
                    let replacement: Vec<String> = joined.split('\n').map(str::to_owned).collect();
                    let inserted_lines = replacement.len();
                    self.lines.splice(self.row..=self.row, replacement);
                    self.row += inserted_lines - 1;
                    self.col = if inserted_lines == 1 {
                        at + text.chars().count().saturating_sub(1)
                    } else {
                        text.rsplit('\n')
                            .next()
                            .unwrap_or("")
                            .chars()
                            .count()
                            .saturating_sub(1)
                    };
                }
                self.mark_changed()
            }
            Action::Undo(count) => {
                for _ in 0..count {
                    self.undo(false)
                }
            }
            Action::Redo(count) => {
                for _ in 0..count {
                    self.undo(true)
                }
            }
            Action::Repeat(count) => self.repeat_last(count),
            Action::SelectRegister(name) => {
                if name.is_ascii_alphanumeric() || matches!(name, '"' | '-' | '_' | '+' | '*') {
                    self.active_register = Some(name);
                } else {
                    self.message = format!("Unsupported register: {name}");
                    self.active_register = None;
                }
            }
            Action::StartMacro(name) => {
                if !name.is_ascii_alphabetic() {
                    self.message = "Macro register must be a letter".into();
                } else if self.macro_depth > 0 {
                    self.message = "Cannot record during macro playback".into();
                } else {
                    let key = name.to_ascii_lowercase();
                    self.macro_keys = if name.is_ascii_uppercase() {
                        self.macros.get(&key).cloned().unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    self.macro_recording = Some(key);
                    self.message = format!("recording @{key}");
                }
            }
            Action::PlayMacro(name, count) => self.play_macro(name, count),
            Action::SetMark(name) => {
                if name.is_ascii_lowercase() {
                    self.marks.insert(name, (self.row, self.col));
                    self.message = format!("mark {name} set");
                } else {
                    self.message = "Local mark must be a-z".into();
                }
            }
            Action::JumpMark(name, linewise) => {
                if let Some(&(row, col)) = self.marks.get(&name) {
                    let row = row.min(self.lines.len() - 1);
                    let col = if linewise {
                        self.lines[row]
                            .chars()
                            .take_while(|c| c.is_whitespace())
                            .count()
                    } else {
                        col.min(self.lines[row].chars().count().saturating_sub(1))
                    };
                    self.record_jump((row, col));
                    (self.row, self.col) = (row, col);
                } else {
                    self.message = format!("Mark {name} not set");
                }
            }
            Action::Search(forward) => {
                self.search_origin = Some((self.row, self.col));
                self.search_before = self.search.clone();
                self.highlight_before = self.highlight;
                self.mode = Mode::Search(forward);
                self.prompt.clear()
            }
            Action::SearchWord(forward) => self.search_word(forward),
            Action::Command => {
                self.mode = Mode::Command;
                self.prompt.clear()
            }
            Action::RepeatSearch(same) => {
                if let Some((_, forward)) = self.search.as_ref() {
                    self.search_next(if same { *forward } else { !forward })
                }
            }
            Action::ClearSearch => {
                self.highlight = false;
                self.message.clear()
            }
            Action::Visual(kind) => {
                self.visual_anchor = (self.row, self.col);
                self.mode = if kind == 'V' {
                    Mode::VisualLine
                } else {
                    Mode::Visual
                };
            }
            Action::Replace(one, count) => {
                if !one {
                    self.snapshot();
                }
                self.replace_count = count;
                self.mode = if one { Mode::ReplaceOne } else { Mode::Replace }
            }
        }
    }
    fn handle_normal(&mut self, key: Key) {
        match key {
            Key::Char(c) => {
                if !self.replaying_change && c != '.' {
                    self.pending_change.push(key);
                }
                if let Some(action) = self.commands.feed(c) {
                    if self.mode == Mode::OperatorPending {
                        self.mode = Mode::Normal
                    }
                    let before = self.revision;
                    self.apply(action);
                    if !matches!(
                        action,
                        Action::SelectRegister(_)
                            | Action::Operate(_, _, _)
                            | Action::Lines(_, _)
                            | Action::TextObject(_, _, _, _)
                            | Action::Put(_, _)
                    ) {
                        self.active_register = None;
                    }
                    if !self.replaying_change {
                        if matches!(action, Action::SelectRegister(_)) {
                            // The register prefix belongs to the next command.
                        } else if Self::repeatable(action) {
                            if matches!(self.mode, Mode::Insert | Mode::Replace | Mode::ReplaceOne)
                            {
                                self.recording_change = true;
                                self.recording_start_revision = before;
                            } else if self.revision != before {
                                self.last_change = Some(self.pending_change.clone());
                                self.pending_change.clear();
                            } else {
                                self.pending_change.clear();
                            }
                        } else {
                            self.pending_change.clear();
                        }
                    }
                } else if self.commands.operator_pending() {
                    self.mode = Mode::OperatorPending;
                } else {
                    self.mode = Mode::Normal;
                    if !self.commands.has_pending() {
                        self.pending_change.clear();
                    }
                }
            }
            Key::Ctrl('v') => {
                self.visual_anchor = (self.row, self.col);
                self.mode = Mode::VisualBlock
            }
            Key::Ctrl('p') => self.open_picker(),
            Key::Delete => self.handle_normal(Key::Char('x')),
            Key::Ctrl('r') => self.undo(true),
            Key::Ctrl('o') => self.jump_history(false),
            Key::Ctrl('i') => self.jump_history(true),
            Key::Left => self.step(Motion::Left),
            Key::Right => self.step(Motion::Right),
            Key::Home => self.step(Motion::Start),
            Key::End => self.step(Motion::End),
            Key::Down => {
                if self.highlight && self.search.is_some() {
                    self.search_next(true)
                } else {
                    self.step(Motion::Down)
                }
            }
            Key::Up => {
                if self.highlight && self.search.is_some() {
                    self.search_next(false)
                } else {
                    self.step(Motion::Up)
                }
            }
            Key::Esc => {
                self.commands.reset();
                self.pending_change.clear();
                self.active_register = None;
                self.mode = Mode::Normal;
                self.apply(Action::ClearSearch)
            }
            _ => (),
        }
    }
    pub fn handle(&mut self, key: Key) {
        let start_insert_once = self.mode == Mode::Insert && key == Key::Ctrl('o');
        if self.macro_depth == 0 {
            if let Some(name) = self.macro_recording {
                if self.mode == Mode::Normal && key == Key::Char('q') {
                    self.macros
                        .insert(name, std::mem::take(&mut self.macro_keys));
                    self.macro_recording = None;
                    self.message = format!("recorded @{name}");
                    return;
                }
                if self.macro_keys.len() >= 10_000 {
                    self.macro_recording = None;
                    self.macro_keys.clear();
                    self.message = "Macro exceeds 10000 keys".into();
                } else {
                    self.macro_keys.push(key);
                }
            }
        }
        let recording_key = self.recording_change
            && matches!(self.mode, Mode::Insert | Mode::Replace | Mode::ReplaceOne);
        if recording_key && !self.replaying_change {
            self.pending_change.push(key);
        }
        match self.mode {
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock => self.visual_key(key),
            Mode::Replace | Mode::ReplaceOne => self.replace_key(key),
            Mode::Insert => match key {
                Key::Ctrl('o') => {
                    self.insert_once = true;
                    self.mode = Mode::Normal;
                }
                Key::Esc => {
                    self.mode = Mode::Normal;
                    self.col = self.col.saturating_sub(1);
                    self.clamp()
                }
                Key::Char(c) if !self.readonly => self.insert(c),
                Key::Ctrl('i') if !self.readonly => self.insert('\t'),
                Key::Enter if !self.readonly => self.enter(),
                Key::Backspace if !self.readonly => self.backspace(),
                Key::Delete if !self.readonly => self.delete_insert(),
                Key::Ctrl('w') if !self.readonly => self.delete_insert_prefix(true),
                Key::Ctrl('u') if !self.readonly => self.delete_insert_prefix(false),
                Key::Ctrl('t') if !self.readonly => self.insert_shift(true),
                Key::Ctrl('d') if !self.readonly => self.insert_shift(false),
                Key::Left => self.col = self.col.saturating_sub(1),
                Key::Right => self.col = (self.col + 1).min(self.chars().len()),
                Key::Home => self.col = 0,
                Key::End => self.col = self.chars().len(),
                Key::Up => self.move_vertical(false),
                Key::Down => self.move_vertical(true),
                Key::Ctrl(_) => (),
                _ => (),
            },
            Mode::Search(forward) => match key {
                Key::Esc => self.finish_search(forward, false),
                Key::Enter => self.finish_search(forward, true),
                Key::Down => self.cycle_search_preview(true),
                Key::Up => self.cycle_search_preview(false),
                Key::Backspace => {
                    self.prompt.pop();
                    self.preview_search(forward);
                }
                Key::Char(c) => {
                    self.prompt.push(c);
                    self.preview_search(forward);
                }
                _ => (),
            },
            Mode::Command => match key {
                Key::Esc => {
                    self.mode = Mode::Normal;
                    self.prompt.clear()
                }
                Key::Enter => {
                    self.command();
                    if self.mode == Mode::Command {
                        self.mode = Mode::Normal
                    }
                    self.prompt.clear()
                }
                Key::Backspace => {
                    self.prompt.pop();
                }
                Key::Char(c) => self.prompt.push(c),
                _ => (),
            },
            Mode::Normal | Mode::OperatorPending => self.handle_normal(key),
            Mode::Explorer => {
                if key == Key::Ctrl('p') {
                    self.open_picker()
                } else {
                    self.explorer_key(key)
                }
            }
            Mode::Picker => self.picker_key(key),
            Mode::Quickfix => self.quickfix_key(key),
            Mode::Completion => self.completion_key(key),
            Mode::CodeActions => self.code_action_key(key),
            Mode::GitStatus => self.git_status_key(key),
            Mode::GitDiff => match key {
                Key::Char('j') | Key::Down => {
                    self.row = (self.row + 1).min(self.git_diff_lines.len().saturating_sub(1))
                }
                Key::Char('k') | Key::Up => self.row = self.row.saturating_sub(1),
                Key::Char('q') | Key::Esc => {
                    self.mode = Mode::Normal;
                    (self.row, self.col) = self.git_previous_cursor
                }
                _ => (),
            },
        }
        if recording_key && self.mode == Mode::Normal && !self.replaying_change {
            if self.revision != self.recording_start_revision {
                self.last_change = Some(self.pending_change.clone());
            }
            self.pending_change.clear();
            self.recording_change = false;
        }
        if self.insert_once && !start_insert_once && !self.commands.has_pending() {
            match self.mode {
                Mode::Normal => {
                    self.insert_once = false;
                    self.mode = Mode::Insert;
                }
                Mode::Insert => self.insert_once = false,
                _ => (),
            }
        }
    }
}

fn ex_address(input: &str, current: usize, total: usize) -> Option<(usize, &str)> {
    if let Some(rest) = input.strip_prefix('.') {
        return Some((current + 1, rest));
    }
    if let Some(rest) = input.strip_prefix('$') {
        return Some((total, rest));
    }
    let digits = input.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0)
        .then(|| {
            input[..digits]
                .parse()
                .ok()
                .map(|line| (line, &input[digits..]))
        })
        .flatten()
}
pub fn vim_regex(pattern: &str, case_insensitive: bool) -> Result<Regex, regex::Error> {
    let mut translated = String::with_capacity(pattern.len());
    let mut magic = 0u8; // 0 = magic, 1 = very magic, 2 = nomagic, 3 = very nomagic
    let mut escaped = false;
    let mut in_class = false;
    for ch in pattern.chars() {
        if escaped {
            escaped = false;
            if in_class {
                translated.push('\\');
                translated.push(ch);
                continue;
            }
            match ch {
                'v' => magic = 1,
                'm' => magic = 0,
                'M' => magic = 2,
                'V' => magic = 3,
                'c' => translated.push_str("(?i)"),
                'C' => translated.push_str("(?-i)"),
                '<' | '>' => translated.push_str("\\b"),
                '(' | ')' | '+' | '?' | '|' | '{' | '}' if magic != 1 => translated.push(ch),
                '.' | '*' | '^' | '$' if magic >= 2 => translated.push(ch),
                '[' if magic >= 2 => {
                    in_class = true;
                    translated.push(ch);
                }
                '(' | ')' | '+' | '?' | '|' | '{' | '}' if magic == 1 => {
                    translated.push('\\');
                    translated.push(ch);
                }
                '/' => translated.push('/'),
                _ => {
                    translated.push('\\');
                    translated.push(ch);
                }
            }
            continue;
        }
        if ch == '\\' {
            escaped = true;
        } else if ch == '[' && magic <= 1 {
            in_class = true;
            translated.push(ch);
        } else if ch == ']' && in_class {
            in_class = false;
            translated.push(ch);
        } else if !in_class
            && (magic == 3
                && matches!(
                    ch,
                    '^' | '$' | '.' | '*' | '[' | ']' | '(' | ')' | '+' | '?' | '|' | '{' | '}'
                )
                || magic == 2
                    && matches!(
                        ch,
                        '.' | '*' | '[' | ']' | '(' | ')' | '+' | '?' | '|' | '{' | '}'
                    )
                || magic == 0 && matches!(ch, '(' | ')' | '+' | '?' | '|' | '{' | '}'))
        {
            translated.push('\\');
            translated.push(ch);
        } else {
            translated.push(ch);
        }
    }
    if escaped {
        translated.push('\\');
    }
    RegexBuilder::new(&translated)
        .case_insensitive(case_insensitive)
        .build()
}
fn parse_line_command(
    command: &str,
    current: usize,
    total: usize,
) -> Option<Result<(std::ops::Range<usize>, &str, &str), String>> {
    let mut rest = command;
    let mut lines = current..current + 1;
    let mut invalid_range = false;
    if let Some(after) = rest.strip_prefix('%') {
        lines = 0..total;
        rest = after;
    } else if let Some((first, after)) = ex_address(rest, current, total) {
        rest = after;
        let mut last = first;
        if let Some(after_comma) = rest.strip_prefix(',') {
            if let Some((second, after_second)) = ex_address(after_comma, current, total) {
                last = second;
                rest = after_second;
            } else {
                invalid_range = true;
            }
        }
        invalid_range |= first == 0 || last == 0 || first > total || last > total || first > last;
        if !invalid_range {
            lines = first - 1..last;
        }
    }
    let split = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let name = &rest[..split];
    if !matches!(
        name,
        "sort"
            | "sort!"
            | "d"
            | "delete"
            | "y"
            | "yank"
            | "pu"
            | "put"
            | "put!"
            | "undo"
            | "redo"
            | "j"
            | "join"
            | "join!"
            | "m"
            | "move"
            | "t"
            | "copy"
    ) {
        return None;
    }
    if invalid_range {
        return Some(Err("Invalid line range".into()));
    }
    Some(Ok((lines, name, rest[split..].trim())))
}
fn delimited_part(input: &str, delimiter: char) -> (String, Option<&str>) {
    let mut result = String::new();
    let mut chars = input.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch == delimiter {
            let rest = chars.peek().map_or("", |(index, _)| &input[*index..]);
            return (result, Some(rest));
        }
        if ch == '\\' {
            if let Some((_, next)) = chars.peek() {
                if *next == delimiter {
                    result.push(delimiter);
                    chars.next();
                    continue;
                }
            }
        }
        result.push(ch);
    }
    (result, None)
}
fn vim_replacement(input: &str) -> String {
    let mut result = String::new();
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '&' => result.push_str("${0}"),
            '$' => result.push_str("$$"),
            '\\' => match chars.next() {
                Some(digit @ '0'..='9') => {
                    result.push_str("${");
                    result.push(digit);
                    result.push('}');
                }
                Some('r') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some(next) => result.push(next),
                None => result.push('\\'),
            },
            other => result.push(other),
        }
    }
    result
}
fn parse_global(
    command: &str,
    current: usize,
    total: usize,
    previous_pattern: Option<&str>,
) -> Option<Result<GlobalSpec, String>> {
    let mut rest = command;
    let mut lines = 0..total;
    let mut invalid_range = false;
    if let Some(after) = rest.strip_prefix('%') {
        rest = after;
    } else if let Some((first, after)) = ex_address(rest, current, total) {
        rest = after;
        let mut last = first;
        if let Some(after_comma) = rest.strip_prefix(',') {
            if let Some((second, after_second)) = ex_address(after_comma, current, total) {
                last = second;
                rest = after_second;
            } else {
                invalid_range = true;
            }
        }
        invalid_range |= first == 0 || last == 0 || first > total || last > total || first > last;
        if !invalid_range {
            lines = first - 1..last;
        }
    }
    let (inverse, body) = if let Some(body) = rest.strip_prefix("vglobal") {
        (true, body)
    } else if let Some(body) = rest.strip_prefix("global") {
        (false, body)
    } else if let Some(body) = rest.strip_prefix("g!") {
        (true, body)
    } else if let Some(body) = rest.strip_prefix('v') {
        (true, body)
    } else if let Some(body) = rest.strip_prefix('g') {
        (false, body)
    } else {
        return None;
    };
    let Some(delimiter) = body.chars().next() else {
        return Some(Err("Global pattern required".into()));
    };
    if delimiter.is_ascii_alphanumeric() || delimiter.is_whitespace() || delimiter == '\\' {
        return None;
    }
    if invalid_range {
        return Some(Err("Invalid global range".into()));
    }
    let (pattern, command) = delimited_part(&body[delimiter.len_utf8()..], delimiter);
    let Some(operation) = command else {
        return Some(Err("Unterminated global pattern".into()));
    };
    let pattern = if pattern.is_empty() {
        match previous_pattern {
            Some(pattern) => pattern.to_owned(),
            None => return Some(Err("No previous search pattern".into())),
        }
    } else {
        pattern
    };
    Some(Ok(GlobalSpec {
        lines,
        pattern,
        inverse,
        operation: operation.trim().to_owned(),
    }))
}
fn parse_substitute(
    command: &str,
    current: usize,
    total: usize,
    previous_pattern: Option<&str>,
) -> Option<Result<SubstituteSpec, String>> {
    let mut rest = command;
    let mut lines = current..current + 1;
    let mut invalid_range = false;
    if let Some(after) = rest.strip_prefix('%') {
        lines = 0..total;
        rest = after;
    } else if let Some((first, after)) = ex_address(rest, current, total) {
        rest = after;
        let mut last = first;
        if let Some(after_comma) = rest.strip_prefix(',') {
            if let Some((second, after_second)) = ex_address(after_comma, current, total) {
                last = second;
                rest = after_second;
            } else {
                invalid_range = true;
            }
        }
        invalid_range |= first == 0 || last == 0 || first > total || last > total || first > last;
        if !invalid_range {
            lines = first - 1..last;
        }
    }
    let body = rest
        .strip_prefix("substitute")
        .or_else(|| rest.strip_prefix('s'))?;
    let Some(delimiter) = body.chars().next() else {
        return Some(Err("Substitute delimiter required".into()));
    };
    if delimiter.is_ascii_alphanumeric() || delimiter.is_whitespace() || delimiter == '\\' {
        return None;
    }
    if invalid_range {
        return Some(Err("Invalid substitute range".into()));
    }
    let body = &body[delimiter.len_utf8()..];
    let (pattern, after_pattern) = delimited_part(body, delimiter);
    let Some(after_pattern) = after_pattern else {
        return Some(Err("Missing substitute replacement".into()));
    };
    let (replacement, flags) = delimited_part(after_pattern, delimiter);
    let flags = flags.unwrap_or("");
    let pattern = if pattern.is_empty() {
        match previous_pattern {
            Some(previous) => previous.to_owned(),
            None => return Some(Err("No previous search pattern".into())),
        }
    } else {
        pattern
    };
    let mut spec = SubstituteSpec {
        lines: lines.collect(),
        pattern,
        replacement: vim_replacement(&replacement),
        global: false,
        case_insensitive: None,
        suppress_missing: false,
        count_only: false,
    };
    for flag in flags.chars() {
        match flag {
            'g' => spec.global = true,
            'i' => spec.case_insensitive = Some(true),
            'I' => spec.case_insensitive = Some(false),
            'e' => spec.suppress_missing = true,
            'n' => spec.count_only = true,
            _ => return Some(Err(format!("Unsupported substitute flag: {flag}"))),
        }
    }
    Some(Ok(spec))
}
fn utf16_position_to_byte(text: &str, (row, col): (usize, usize)) -> Option<usize> {
    let mut offset = 0;
    for (index, line) in text.split('\n').enumerate() {
        if index == row {
            let mut units = 0;
            for (byte, ch) in line.char_indices() {
                if units == col {
                    return Some(offset + byte);
                }
                units += ch.len_utf16();
                if units > col {
                    return None;
                }
            }
            return (units == col).then_some(offset + line.len());
        }
        offset += line.len() + 1;
    }
    None
}

fn detect_project_root(path: &std::path::Path) -> Option<PathBuf> {
    let start = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let absolute = start.canonicalize().ok()?;
    const MARKERS: [&str; 5] = [
        ".git",
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
    ];
    absolute
        .ancestors()
        .find(|directory| MARKERS.iter().any(|marker| directory.join(marker).exists()))
        .map(PathBuf::from)
}

fn hash_bytes(data: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn journal_recovers_unsaved_text_and_persistent_undo() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("draft.txt");
        let journal = dir.path().join("journal.json");
        fs::write(&file, "before\n").unwrap();
        let mut edited = Editor::open(Some(file.clone()), false).unwrap();
        for key in [Key::Char('i'), Key::Char('X'), Key::Esc] {
            edited.handle(key);
        }
        edited.persist_journal_at(&journal).unwrap();
        let mut reopened = Editor::open(Some(file.clone()), false).unwrap();
        assert!(reopened.restore_journal_at(&journal).unwrap());
        assert_eq!(reopened.lines[0], "Xbefore");
        assert!(reopened.modified);
        reopened.handle(Key::Char('u'));
        assert_eq!(reopened.lines[0], "before");
        reopened.handle(Key::Ctrl('r'));
        assert_eq!(reopened.lines[0], "Xbefore");

        fs::write(&file, "external\n").unwrap();
        let mut changed = Editor::open(Some(file), false).unwrap();
        assert!(!changed.restore_journal_at(&journal).unwrap());
        assert_eq!(changed.lines[0], "external");
    }
    #[test]
    fn clipboard_register_preserves_linewise_text() {
        let register = StoredRegister::from_text("first\nsecond\n");
        assert!(register.linewise);
        assert_eq!(register.lines, ["first", "second"]);
        assert_eq!(register.text(), "first\nsecond\n");
        let chars = StoredRegister::from_text("one\ntwo");
        assert!(!chars.linewise);
        assert_eq!(chars.text(), "one\ntwo");
    }
    #[test]
    fn local_marks_and_jump_history() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["  first".into(), "middle".into(), "  last".into()];
        e.col = 4;
        for c in "maG`a".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!((e.row, e.col), (0, 4));
        e.handle(Key::Ctrl('o'));
        assert_eq!(e.row, 2);
        e.handle(Key::Ctrl('i'));
        assert_eq!((e.row, e.col), (0, 4));
        for c in "G'a".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!((e.row, e.col), (0, 2));
    }
    #[test]
    fn tab_key_inserts_literal_tab_in_insert_mode() {
        let mut e = Editor::open(None, false).unwrap();
        e.handle(Key::Char('i'));
        e.handle(Key::Ctrl('i'));
        e.handle(Key::Esc);
        assert_eq!(e.lines[0], "\t");
    }
    #[test]
    fn code_action_panel_applies_selected_edit_and_undo() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["let x = ;".into()];
        e.col = 8;
        e.show_code_actions(vec![CodeAction {
            title: "Insert value".into(),
            edits: vec![TextEdit {
                start: (0, 8),
                end: (0, 8),
                new_text: "1".into(),
            }],
        }]);
        assert_eq!(e.mode, Mode::CodeActions);
        e.handle(Key::Enter);
        assert_eq!(e.lines[0], "let x = 1;");
        e.handle(Key::Char('u'));
        assert_eq!(e.lines[0], "let x = ;");
    }
    #[test]
    fn lsp_edits_use_utf16_and_preserve_undo_state() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["a😀b".into()];
        assert_eq!(e.content(), "a😀b\n");
        let edit = TextEdit {
            start: (0, 1),
            end: (0, 3),
            new_text: "é".into(),
        };
        assert_eq!(e.apply_lsp_edits(&[edit]).unwrap(), 1);
        assert_eq!(e.content(), "aéb\n");
        e.handle(Key::Char('u'));
        assert_eq!(e.content(), "a😀b\n");
        let edit = TextEdit {
            start: (0, 0),
            end: (1, 0),
            new_text: "xyz".into(),
        };
        e.apply_lsp_edits(&[edit]).unwrap();
        assert_eq!(e.content(), "xyz");
        e.handle(Key::Char('u'));
        assert_eq!(e.content(), "a😀b\n");
    }
    #[test]
    fn lsp_edits_reject_invalid_ranges_without_mutation() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["a😀b".into()];
        let bad = TextEdit {
            start: (0, 2),
            end: (0, 3),
            new_text: "x".into(),
        };
        assert!(e.apply_lsp_edits(&[bad]).is_err());
        assert_eq!(e.content(), "a😀b\n");
        assert!(!e.modified);
    }
    #[test]
    fn macro_record_replay_repeat_and_append() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["one".into(), "two".into(), "three".into()];
        for key in [
            Key::Char('q'),
            Key::Char('a'),
            Key::Char('A'),
            Key::Char('!'),
            Key::Esc,
            Key::Char('q'),
            Key::Char('j'),
            Key::Char('@'),
            Key::Char('a'),
            Key::Char('@'),
            Key::Char('@'),
        ] {
            e.handle(key);
        }
        assert_eq!(e.lines, ["one!", "two!!", "three"]);
        for key in [
            Key::Char('q'),
            Key::Char('A'),
            Key::Char('A'),
            Key::Char('?'),
            Key::Esc,
            Key::Char('q'),
            Key::Char('j'),
            Key::Char('@'),
            Key::Char('a'),
        ] {
            e.handle(key);
        }
        assert_eq!(e.lines[2], "three!?");
    }
    #[test]
    fn named_registers_keep_independent_yanks_and_put() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["one".into(), "two".into(), "three".into()];
        for c in "\"ayyj\"byyG\"ap\"bp".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines, ["one", "two", "three", "one", "two"]);
        assert_eq!(e.named_registers.get(&'a').unwrap().lines, ["one"]);
        assert_eq!(e.named_registers.get(&'b').unwrap().lines, ["two"]);
        for c in "\"Ayy".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.named_registers.get(&'a').unwrap().lines, ["one", "two"]);
    }
    #[test]
    fn dot_repeats_insert_operator_and_count() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["xyz".into()];
        for key in [Key::Char('a'), Key::Char('!'), Key::Esc, Key::Char('.')] {
            e.handle(key);
        }
        assert_eq!(e.lines[0], "x!!yz");

        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["one two three four".into()];
        for c in "dw2.".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines[0], "four");
        e.handle(Key::Char('u'));
        e.handle(Key::Char('.'));
        assert_eq!(e.lines[0], "four");
    }
    #[test]
    fn star_and_hash_search_whole_word() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["cat scatter cat".into(), "cat".into()];
        e.handle(Key::Char('*'));
        assert_eq!((e.row, e.col), (0, 12));
        e.handle(Key::Char('*'));
        assert_eq!((e.row, e.col), (1, 0));
        e.handle(Key::Char('#'));
        assert_eq!((e.row, e.col), (0, 12));
    }
    #[test]
    fn page_scroll_clamps_to_buffer() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = (0..30).map(|n| format!("line {n}")).collect();
        e.scroll_rows(true, 20);
        assert_eq!(e.row, 20);
        e.scroll_rows(true, 20);
        assert_eq!(e.row, 29);
        e.scroll_rows(false, 100);
        assert_eq!(e.row, 0);
    }
    #[test]
    fn character_find_repeats_and_reverses_on_one_line() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["a,b,c,d".into()];
        for c in "2f,".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.col, 3);
        e.handle(Key::Char(';'));
        assert_eq!(e.col, 5);
        e.handle(Key::Char(','));
        assert_eq!(e.col, 3);
        e.col = 0;
        for c in "t,".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.col, 0);
        e.handle(Key::Char(';'));
        assert_eq!(e.col, 2);
        e.handle(Key::Char(';'));
        assert_eq!(e.col, 4);
    }
    #[test]
    fn find_is_composable_with_delete_and_undo() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["one,two,three".into()];
        for c in "df,".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines[0], "two,three");
        e.handle(Key::Char('u'));
        assert_eq!(e.lines[0], "one,two,three");
        for c in "dt,".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines[0], ",two,three");
        e.handle(Key::Char('u'));
        e.col = 6;
        for c in "dF,".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines[0], "one,three");
    }
    #[test]
    fn percent_matches_nested_brackets_and_operator_across_lines() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["(a(b)c)".into()];
        e.handle(Key::Char('%'));
        assert_eq!((e.row, e.col), (0, 6));
        e.handle(Key::Char('%'));
        assert_eq!((e.row, e.col), (0, 0));
        e.lines = vec![
            "fn a() {".into(),
            "  if (x) {".into(),
            "    x".into(),
            "  }".into(),
            "}".into(),
        ];
        e.col = 7;
        e.handle(Key::Char('%'));
        assert_eq!((e.row, e.col), (4, 0));
        e.row = 0;
        e.col = 7;
        for c in "d%".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines, ["fn a() "]);
        e.handle(Key::Char('u'));
        assert_eq!(e.lines.len(), 5);
    }
    #[test]
    fn paragraph_motions_follow_empty_line_boundaries() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec![
            "aa".into(),
            "bb".into(),
            "".into(),
            "cc".into(),
            "dd".into(),
            "".into(),
            "ee".into(),
        ];
        e.handle(Key::Char('}'));
        assert_eq!(e.row, 2);
        e.handle(Key::Char('}'));
        assert_eq!(e.row, 5);
        e.handle(Key::Char('}'));
        assert_eq!(e.row, 6);
        e.handle(Key::Char('{'));
        assert_eq!(e.row, 5);
        e.handle(Key::Char('{'));
        assert_eq!(e.row, 2);
        e.handle(Key::Char('{'));
        assert_eq!(e.row, 0);
    }
    #[test]
    fn screen_motions_use_visible_rows_and_counts() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = (0..30).map(|n| format!("  line {n}")).collect();
        e.set_viewport(10, 10);
        e.handle(Key::Char('H'));
        assert_eq!((e.row, e.col), (10, 2));
        for c in "3H".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.row, 12);
        e.handle(Key::Char('M'));
        assert_eq!(e.row, 15);
        e.handle(Key::Char('L'));
        assert_eq!(e.row, 19);
        for c in "2L".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.row, 18);
    }
    #[test]
    fn indent_commands_support_counts_undo_repeat_and_visual_lines() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["one".into(), "two".into(), "".into(), "three".into()];
        for c in "2>>".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines, ["    one", "    two", "", "three"]);
        e.handle(Key::Char('u'));
        assert_eq!(e.lines[0], "one");
        for c in "2>>".chars() {
            e.handle(Key::Char(c));
        }
        e.handle(Key::Char('.'));
        assert_eq!(e.lines[0], "        one");
        for c in "2<<".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines[0], "    one");
        e.handle(Key::Char('V'));
        e.handle(Key::Char('j'));
        e.handle(Key::Char('<'));
        assert_eq!(e.lines[0], "one");
        assert_eq!(e.lines[1], "two");
    }
    #[test]
    fn paragraph_objects_and_shift_motions_compose() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec![
            "aa".into(),
            "bb".into(),
            "".into(),
            "cc".into(),
            "dd".into(),
            "".into(),
            "ee".into(),
        ];
        e.row = 3;
        for c in "dip".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines, ["aa", "bb", "", "", "ee"]);
        e.handle(Key::Char('u'));
        for c in "dap".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines, ["aa", "bb", "", "ee"]);
        e.handle(Key::Char('u'));
        e.row = 0;
        for c in ">j".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(&e.lines[..2], ["    aa", "    bb"]);
        e.handle(Key::Char('u'));
        for c in ">ip".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(&e.lines[..2], ["    aa", "    bb"]);
        e.handle(Key::Char('u'));
        e.row = 3;
        for c in "2dip".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines, ["aa", "bb", "", "ee"]);
    }
    #[test]
    fn counted_replace_matches_vim_without_partial_write() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["abcdef".into()];
        e.col = 1;
        for c in "3rX".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines[0], "aXXXef");
        assert_eq!(e.col, 3);
        e.handle(Key::Char('u'));
        assert_eq!(e.lines[0], "abcdef");
        for c in "3rX".chars() {
            e.handle(Key::Char(c));
        }
        e.col = 2;
        e.handle(Key::Char('.'));
        assert_eq!(e.lines[0], "aXXXXf");
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["abcd".into()];
        e.col = 2;
        for c in "3rX".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines[0], "abcd");
        assert!(!e.modified);
    }
    #[test]
    fn counted_replace_with_enter_splits_at_removed_span() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["abcdef".into()];
        e.col = 1;
        for key in [Key::Char('3'), Key::Char('r'), Key::Enter] {
            e.handle(key);
        }
        assert_eq!(e.lines, ["a", "ef"]);
        assert_eq!((e.row, e.col), (1, 0));
        e.handle(Key::Char('u'));
        assert_eq!(e.lines, ["abcdef"]);
    }
    #[test]
    fn backward_search_and_arrows() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec![
            "one".into(),
            "other".into(),
            "one".into(),
            "other".into(),
            "one".into(),
        ];
        e.row = 4;
        for k in [
            Key::Char('?'),
            Key::Char('o'),
            Key::Char('n'),
            Key::Char('e'),
            Key::Enter,
        ] {
            e.handle(k)
        }
        assert_eq!(e.row, 2);
        e.handle(Key::Down);
        assert_eq!(e.row, 4);
        e.handle(Key::Up);
        assert_eq!(e.row, 2);
        e.handle(Key::Char('n'));
        assert_eq!(e.row, 0);
        e.handle(Key::Char('N'));
        assert_eq!(e.row, 2);
        e.handle(Key::Esc);
        e.handle(Key::Down);
        assert_eq!(e.row, 3);
    }
    #[test]
    fn incremental_search_previews_cancels_and_records_one_jump() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["alpha".into(), "beta".into(), "alpine".into()];
        e.row = 1;
        e.handle(Key::Char('/'));
        e.handle(Key::Char('a'));
        assert_eq!((e.row, e.col), (1, 3));
        e.handle(Key::Char('l'));
        assert_eq!((e.row, e.col), (2, 0));
        e.handle(Key::Backspace);
        assert_eq!((e.row, e.col), (1, 3));
        e.handle(Key::Esc);
        assert_eq!((e.row, e.col), (1, 0));
        assert!(e.search.is_none());
        assert!(e.jump_back.is_empty());
        for key in [Key::Char('/'), Key::Char('a'), Key::Char('l'), Key::Enter] {
            e.handle(key);
        }
        assert_eq!((e.row, e.col), (2, 0));
        assert_eq!(e.jump_back, [(1, 0)]);
        e.handle(Key::Ctrl('o'));
        assert_eq!((e.row, e.col), (1, 0));
        e.handle(Key::Char('/'));
        e.handle(Key::Char('['));
        assert_eq!((e.row, e.col), (1, 0));
        e.handle(Key::Enter);
        assert_eq!(e.search.as_ref().unwrap().0, "al");
    }
    #[test]
    fn search_arrows_preview_then_enter_commits_selected_match() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["one".into(), "middle".into(), "one".into(), "one".into()];
        e.row = 3;
        for key in [
            Key::Char('?'),
            Key::Char('o'),
            Key::Char('n'),
            Key::Char('e'),
        ] {
            e.handle(key);
        }
        assert_eq!(e.row, 2);
        e.handle(Key::Up);
        assert_eq!(e.row, 0);
        e.handle(Key::Down);
        assert_eq!(e.row, 2);
        e.handle(Key::Enter);
        assert_eq!(e.row, 2);
        assert_eq!(e.jump_back, [(3, 0)]);
        e.handle(Key::Down);
        assert_eq!(e.row, 3);
    }
    #[test]
    fn insert_and_command() {
        let mut e = Editor::open(None, false).unwrap();
        for k in [
            Key::Char('i'),
            Key::Char('é'),
            Key::Enter,
            Key::Char('x'),
            Key::Esc,
        ] {
            e.handle(k)
        }
        assert_eq!(e.lines, ["é", "x"]);
        e.handle(Key::Char(':'));
        for c in "q".chars() {
            e.handle(Key::Char(c))
        }
        e.handle(Key::Enter);
        assert!(!e.quit);
    }
    #[test]
    fn insert_mode_autoindent_and_ctrl_editing() {
        let mut e = Editor::open(None, false).unwrap();
        e.autoindent = true;
        e.shiftwidth = 2;
        e.lines = vec!["  alpha beta".into()];
        e.row = 0;
        e.col = e.lines[0].chars().count();
        e.begin_insert();
        e.handle(Key::Enter);
        assert_eq!(e.lines, ["  alpha beta", "  "]);
        assert_eq!(e.col, 2);
        for ch in "foo bar".chars() {
            e.handle(Key::Char(ch));
        }
        e.handle(Key::Ctrl('w'));
        assert_eq!(e.lines[1], "  foo ");
        e.handle(Key::Ctrl('t'));
        assert_eq!(e.lines[1], "    foo ");
        e.handle(Key::Ctrl('d'));
        assert_eq!(e.lines[1], "  foo ");
        e.handle(Key::Ctrl('u'));
        assert_eq!(e.lines[1], "");
        e.handle(Key::Esc);
        e.handle(Key::Char('u'));
        assert_eq!(e.lines, ["  alpha beta"]);
    }
    #[test]
    fn insert_ctrl_o_runs_one_composed_normal_command() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["alpha beta".into(), "other".into()];
        e.begin_insert();
        e.handle(Key::Ctrl('o'));
        assert_eq!(e.mode, Mode::Normal);
        e.handle(Key::Char('d'));
        assert_eq!(e.mode, Mode::OperatorPending);
        e.handle(Key::Char('w'));
        assert_eq!(e.mode, Mode::Insert);
        assert_eq!(e.lines[0], "beta");
        e.handle(Key::Ctrl('o'));
        e.handle(Key::Char('j'));
        assert_eq!(e.mode, Mode::Insert);
        assert_eq!(e.row, 1);
    }
    #[test]
    fn change_list_tracks_edit_groups_and_moves_with_g_semicolon_comma() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["alpha".into(), "beta".into(), "gamma".into()];
        e.handle(Key::Char('x'));
        e.handle(Key::Char('G'));
        e.handle(Key::Char('x'));
        assert_eq!(e.change_list, [(0, 0), (2, 0)]);
        for key in "g;g;".chars() {
            e.handle(Key::Char(key));
        }
        assert_eq!(e.row, 0);
        for key in "g,".chars() {
            e.handle(Key::Char(key));
        }
        assert_eq!(e.row, 2);
    }
    #[test]
    fn special_registers_track_yanks_deletions_and_black_hole() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["alpha".into(), "beta".into(), "gamma".into()];
        for ch in "yydd".chars() {
            e.handle(Key::Char(ch));
        }
        assert_eq!(e.named_registers[&'0'].lines, ["alpha"]);
        assert_eq!(e.named_registers[&'1'].lines, ["alpha"]);
        for ch in "dd".chars() {
            e.handle(Key::Char(ch));
        }
        assert_eq!(e.named_registers[&'1'].lines, ["beta"]);
        assert_eq!(e.named_registers[&'2'].lines, ["alpha"]);
        let unnamed = e.register.clone();
        for ch in "\"_x".chars() {
            e.handle(Key::Char(ch));
        }
        assert_eq!(e.register, unnamed);
        assert_eq!(e.lines, ["amma"]);
        e.handle(Key::Char('x'));
        assert_eq!(e.named_registers[&'-'].lines, ["a"]);
    }
    #[test]
    fn line_operators_and_undo() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["alpha".into(), "beta".into(), "gamma".into()];
        for c in "2dd".chars() {
            e.handle(Key::Char(c))
        }
        assert_eq!(e.lines, ["gamma"]);
        e.handle(Key::Char('u'));
        assert_eq!(e.lines, ["alpha", "beta", "gamma"]);
        for c in "yyp".chars() {
            e.handle(Key::Char(c))
        }
        assert_eq!(e.lines, ["alpha", "alpha", "beta", "gamma"]);
        e.handle(Key::Char('u'));
        assert_eq!(e.lines, ["alpha", "beta", "gamma"]);
    }
    #[test]
    fn additional_vim_motions_and_edits() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["  alpha,beta gamma".into(), "next line".into()];
        for c in "^w".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.col, 7); // punctuation and words are separate for w
        e.handle(Key::Char('W'));
        assert_eq!(e.col, 13);
        e.handle(Key::Char('B'));
        assert_eq!(e.col, 2);
        for c in "de".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines[0], "  ,beta gamma");
        e.handle(Key::Char('u'));
        assert_eq!(e.lines[0], "  alpha,beta gamma");
        for c in "A!".chars() {
            e.handle(Key::Char(c));
        }
        e.handle(Key::Esc);
        assert_eq!(e.lines[0], "  alpha,beta gamma!");
        e.handle(Key::Char('J'));
        assert_eq!(e.lines, ["  alpha,beta gamma! next line"]);
        e.handle(Key::Char('0'));
        e.handle(Key::Char('^'));
        e.handle(Key::Char('~'));
        assert_eq!(e.lines[0], "  Alpha,beta gamma! next line");
    }
    #[test]
    fn text_objects_word_quote_and_parentheses() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["alpha beta gamma".into()];
        e.col = 7;
        for c in "diw".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines[0], "alpha  gamma");
        e.handle(Key::Char('u'));
        e.col = 7;
        for c in "yiw".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.col, 7);
        for c in "daw".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines[0], "alpha gamma");

        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["call(\"old\", nested(x))".into()];
        e.col = 7;
        for c in "ci\"".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines[0], "call(\"\", nested(x))");
        e.handle(Key::Char('n'));
        e.handle(Key::Esc);
        assert_eq!(e.lines[0], "call(\"n\", nested(x))");
        e.col = 17;
        for c in "da(".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines[0], "call(\"n\", nested)");
        e.col = 16;
        for c in "di(".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines[0], "call()");
    }
    #[test]
    fn delimited_text_objects_span_lines_and_outer_counts() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec![
            "fn main() {".into(),
            "  call(".into(),
            "    alpha,".into(),
            "    beta".into(),
            "  );".into(),
            "}".into(),
        ];
        e.row = 2;
        e.col = 5;
        for ch in "di(".chars() {
            e.handle(Key::Char(ch));
        }
        assert_eq!(e.lines, ["fn main() {", "  call();", "}"]);
        e.handle(Key::Char('u'));
        assert_eq!(e.lines.len(), 6);
        e.lines = vec!["a(b(c)d)e".into()];
        e.row = 0;
        e.col = 4;
        for ch in "2da(".chars() {
            e.handle(Key::Char(ch));
        }
        assert_eq!(e.lines, ["ae"]);
        e.lines = vec![
            "fn main() {".into(),
            "  a();".into(),
            "  b();".into(),
            "}".into(),
        ];
        e.row = 1;
        e.col = 3;
        for ch in ">i{".chars() {
            e.handle(Key::Char(ch));
        }
        assert_eq!(e.lines, ["fn main() {", "      a();", "      b();", "}"]);
    }
    #[test]
    fn completion_accept_and_cancel() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["pri".into()];
        e.handle(Key::Char('A'));
        let position = (e.row, e.col);
        e.show_completions(vec!["print".into(), "println".into()]);
        assert_eq!((e.row, e.col), position);
        e.handle(Key::Down);
        assert_eq!(e.completion_index, 1);
        assert_eq!((e.row, e.col), position);
        e.handle(Key::Enter);
        assert_eq!(e.lines[0], "println");
        assert_eq!(e.mode, Mode::Insert);
        e.show_completions(vec!["other".into()]);
        e.handle(Key::Esc);
        assert_eq!(e.lines[0], "println");
        assert_eq!(e.mode, Mode::Insert);
    }
    #[test]
    fn read_only_blocks_edits_but_allows_navigation_and_yank() {
        let mut e = Editor::open(None, true).unwrap();
        e.lines = vec!["alpha".into(), "beta".into()];
        for c in "ixddjyy".chars() {
            e.handle(Key::Char(c));
        }
        assert_eq!(e.lines, ["alpha", "beta"]);
        assert_eq!(e.row, 1);
        assert_eq!(e.mode, Mode::Normal);
        e.handle(Key::Char(':'));
        for c in "%s/beta/gamma/".chars() {
            e.handle(Key::Char(c));
        }
        e.handle(Key::Enter);
        assert_eq!(e.lines, ["alpha", "beta"]);
        assert!(!e.modified);
    }
    #[test]
    fn lsp_quickfix_restores_cursor() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["one".into(), "two".into()];
        e.row = 1;
        e.col = 2;
        e.show_locations(vec![(PathBuf::from("a.rs"), 0, 0)], "references");
        assert_eq!(e.mode, Mode::Quickfix);
        e.handle(Key::Esc);
        assert_eq!((e.row, e.col, e.mode), (1, 2, Mode::Normal));
    }
    #[test]
    fn atomic_save_refuses_external_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "hello").unwrap();
        let mut e = Editor::open(Some(path.clone()), false).unwrap();
        for c in "A".chars() {
            e.handle(Key::Char(c))
        }
        fs::write(&path, "changed elsewhere").unwrap();
        assert_eq!(e.save().unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&path).unwrap(), "changed elsewhere");
    }
    #[test]
    fn project_directory_opens_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
        let mut e = Editor::open(Some(dir.path().to_path_buf()), false).unwrap();
        assert_eq!(e.mode, Mode::Explorer);
        e.handle(Key::Enter);
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(e.lines[0], "fn main() {}");
    }
    #[test]
    fn regex_substitute_with_range_and_undo() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["cat cat".into(), "cat".into()];
        e.handle(Key::Char(':'));
        for c in "%s/cat/dog/g".chars() {
            e.handle(Key::Char(c))
        }
        e.handle(Key::Enter);
        assert_eq!(e.lines, ["dog dog", "dog"]);
        e.handle(Key::Char('u'));
        assert_eq!(e.lines, ["cat cat", "cat"]);
    }
    #[test]
    fn substitute_accepts_ranges_flags_delimiters_and_vim_replacement() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["Cat cat".into(), "cat / cat".into(), "none".into()];
        for c in ":1,2s#cat#dog#gi".chars() {
            e.handle(Key::Char(c));
        }
        e.handle(Key::Enter);
        assert_eq!(e.lines, ["dog dog", "dog / dog", "none"]);
        e.handle(Key::Char('u'));
        assert_eq!(e.lines, ["Cat cat", "cat / cat", "none"]);
        for c in ":2s/cat/(&)/".chars() {
            e.handle(Key::Char(c));
        }
        e.handle(Key::Enter);
        assert_eq!(e.lines[1], "(cat) / cat");
        e.handle(Key::Char('u'));
        for c in ":%s/cat/dog/gn".chars() {
            e.handle(Key::Char(c));
        }
        e.handle(Key::Enter);
        assert_eq!(e.lines[1], "cat / cat");
        assert_eq!(e.message, "3 matches");
        for c in ":3s/cat/dog/e".chars() {
            e.handle(Key::Char(c));
        }
        e.handle(Key::Enter);
        assert_eq!(e.lines[2], "none");
        e.prompt = r"1s/\(Cat\)/[\1]/".into();
        e.command();
        assert_eq!(e.lines[0], "[Cat] cat");
    }
    #[test]
    fn vim_regex_supports_default_and_very_magic() {
        assert!(vim_regex(r"\(ab\)\+", false).unwrap().is_match("abab"));
        assert!(!vim_regex(r"(ab)+", false).unwrap().is_match("abab"));
        assert!(vim_regex(r"\v(ab)+", false).unwrap().is_match("abab"));
        assert!(vim_regex(r"\<word\>", false).unwrap().is_match("a word!"));
        assert!(vim_regex(r"\V(a+b)", false).unwrap().is_match("(a+b)"));
        assert!(!vim_regex(r"\V(a+b)", false).unwrap().is_match("aaab"));
        assert!(vim_regex(r"\Mfoo\.bar", false).unwrap().is_match("fooXbar"));
    }
    #[test]
    fn ignorecase_smartcase_and_substitute_override() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["Cat cat".into()];
        e.ignorecase = true;
        e.smartcase = true;
        assert!(e.search_case_insensitive("cat"));
        assert!(!e.search_case_insensitive("Cat"));
        assert!(e.search_case_insensitive(r"\vcat"));
        e.prompt = "s/cat/dog/g".into();
        e.command();
        assert_eq!(e.lines, ["dog dog"]);
        e.undo(false);
        e.prompt = "s/cat/dog/gI".into();
        e.command();
        assert_eq!(e.lines, ["Cat dog"]);
    }
    #[test]
    fn ex_line_commands_sort_yank_delete_put_and_undo() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["z".into(), "B".into(), "a".into(), "A".into()];
        e.prompt = "%sort iu".into();
        e.command();
        assert_eq!(e.lines, ["a", "B", "z"]);
        e.prompt = "1,2yank".into();
        e.command();
        assert_eq!(e.register, ["a", "B"]);
        e.prompt = "2delete".into();
        e.command();
        assert_eq!(e.lines, ["a", "z"]);
        e.prompt = "$put".into();
        e.command();
        assert_eq!(e.lines, ["a", "z", "B"]);
        e.prompt = "undo".into();
        e.command();
        assert_eq!(e.lines, ["a", "z"]);
        e.prompt = "redo".into();
        e.command();
        assert_eq!(e.lines, ["a", "z", "B"]);
        e.lines = vec!["  one".into(), "two".into(), "three".into(), "four".into()];
        e.row = 0;
        e.prompt = "1,2join".into();
        e.command();
        assert_eq!(e.lines, ["  one two", "three", "four"]);
        e.prompt = "1copy $".into();
        e.command();
        assert_eq!(e.lines, ["  one two", "three", "four", "  one two"]);
        e.prompt = "2move 4".into();
        e.command();
        assert_eq!(e.lines, ["  one two", "four", "  one two", "three"]);
        e.prompt = "2,3move 2".into();
        e.command();
        assert_eq!(e.message, "Destination inside moved range");
        e.lines = vec!["a".into(), "b".into(), "c".into()];
        e.prompt = "1yank a".into();
        e.command();
        e.prompt = "3put a".into();
        e.command();
        assert_eq!(e.lines, ["a", "b", "c", "a"]);
        e.prompt = "1put! a".into();
        e.command();
        assert_eq!(e.lines, ["a", "a", "b", "c", "a"]);
    }
    #[test]
    fn ex_read_inserts_file_lines_with_undo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snippet.txt");
        fs::write(&path, "alpha\nbeta\n").unwrap();
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["before".into(), "after".into()];
        e.prompt = format!("read {}", path.display());
        e.command();
        assert_eq!(e.lines, ["before", "alpha", "beta", "after"]);
        e.undo(false);
        assert_eq!(e.lines, ["before", "after"]);
    }
    #[test]
    fn global_and_inverse_global_select_lines_with_one_undo_step() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["cat".into(), "dog".into(), "catfish".into(), "bird".into()];
        e.prompt = "g/cat/d".into();
        e.command();
        assert_eq!(e.lines, ["dog", "bird"]);
        assert_eq!(e.register, ["cat", "catfish"]);
        e.undo(false);
        assert_eq!(e.lines, ["cat", "dog", "catfish", "bird"]);
        e.prompt = "v/cat/y".into();
        e.command();
        assert_eq!(e.register, ["dog", "bird"]);
        e.prompt = "2,4g/dog/p".into();
        e.command();
        assert!(e.message.contains("2:dog"));
        e.prompt = "g/cat/s/cat/fox/".into();
        e.command();
        assert_eq!(e.lines, ["fox", "dog", "foxfish", "bird"]);
        e.undo(false);
        assert_eq!(e.lines, ["cat", "dog", "catfish", "bird"]);
    }
    #[test]
    fn fuzzy_picker_opens_project_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn lib() {}\n").unwrap();
        let mut e = Editor::open(Some(dir.path().to_path_buf()), false).unwrap();
        e.handle(Key::Ctrl('p'));
        assert_eq!(e.mode, Mode::Picker);
        for c in "main".chars() {
            e.handle(Key::Char(c))
        }
        assert_eq!(e.picker_results.len(), 1);
        e.handle(Key::Enter);
        assert_eq!(e.mode, Mode::Normal);
        assert!(e.path.as_ref().unwrap().ends_with("main.rs"));
    }
    #[cfg(unix)]
    #[test]
    fn picker_skips_directory_symlink_cycles() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file.txt"), "ok").unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).unwrap();
        let mut e = Editor::open(Some(dir.path().to_path_buf()), false).unwrap();
        e.handle(Key::Ctrl('p'));
        assert_eq!(e.picker_results.len(), 1);
    }
    #[test]
    fn direct_file_detects_project_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='demo'\nversion='0.1.0'\n",
        )
        .unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        let file = dir.path().join("src/main.rs");
        fs::write(&file, "fn main() {}\n").unwrap();
        let mut e = Editor::open(Some(file), false).unwrap();
        assert_eq!(e.project_root_path(), Some(dir.path()));
        e.handle(Key::Ctrl('p'));
        assert!(e
            .picker_results
            .iter()
            .any(|path| path.ends_with("Cargo.toml")));
    }
    #[test]
    fn project_grep_opens_selected_result() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "one\nNEEDLE\nthree\n").unwrap();
        let mut e = Editor::open(Some(dir.path().to_path_buf()), false).unwrap();
        e.open_picker();
        e.handle(Key::Esc);
        e.handle(Key::Char(':'));
        for c in "grep NEEDLE".chars() {
            e.handle(Key::Char(c))
        }
        e.handle(Key::Enter);
        assert_eq!(e.mode, Mode::Quickfix);
        assert_eq!(e.quickfix.len(), 1);
        e.handle(Key::Enter);
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(e.row, 1);
    }
    #[test]
    fn counted_g_and_characterwise_register() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec![
            "first".into(),
            "second".into(),
            "alpha beta".into(),
            "fourth".into(),
        ];
        for c in "3G".chars() {
            e.handle(Key::Char(c))
        }
        assert_eq!(e.row, 2);
        e.col = 5;
        for c in "d0".chars() {
            e.handle(Key::Char(c))
        }
        assert_eq!(e.lines[2], " beta");
        e.handle(Key::Char('u'));
        e.col = 0;
        for c in "ywp".chars() {
            e.handle(Key::Char(c))
        }
        assert!(e.lines[2].starts_with("aalpha"), "{}", e.lines[2]);
    }
    #[test]
    fn visual_delete_yank_and_undo() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["alpha beta".into(), "second".into()];
        e.handle(Key::Char('v'));
        for _ in 0..4 {
            e.handle(Key::Char('l'))
        }
        assert!(e.selection_contains(0, 2));
        e.handle(Key::Char('d'));
        assert_eq!(e.lines[0], " beta");
        e.handle(Key::Char('u'));
        assert_eq!(e.lines[0], "alpha beta");
        e.handle(Key::Char('V'));
        e.handle(Key::Char('j'));
        e.handle(Key::Char('y'));
        assert_eq!(e.register, ["alpha beta", "second"]);
        assert_eq!(e.mode, Mode::Normal);
    }
    #[test]
    fn delete_key_removes_visual_selections_and_insert_character() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["abcd".into(), "ABCD".into(), "tail".into()];
        e.handle(Key::Char('v'));
        e.handle(Key::Right);
        e.handle(Key::Delete);
        assert_eq!(e.lines[0], "cd");
        assert_eq!(e.mode, Mode::Normal);
        e.handle(Key::Char('u'));
        assert_eq!(e.lines[0], "abcd");

        e.handle(Key::Char('V'));
        e.handle(Key::Down);
        e.handle(Key::Delete);
        assert_eq!(e.lines, ["tail"]);
        e.handle(Key::Char('u'));
        assert_eq!(e.lines, ["abcd", "ABCD", "tail"]);

        e.col = 1;
        e.handle(Key::Ctrl('v'));
        e.handle(Key::Down);
        e.handle(Key::Delete);
        assert_eq!(e.lines, ["acd", "ACD", "tail"]);
        e.handle(Key::Char('u'));

        e.row = 0;
        e.col = 1;
        e.handle(Key::Char('i'));
        e.handle(Key::Delete);
        assert_eq!(e.lines[0], "acd");
        e.handle(Key::Esc);
        e.handle(Key::Char('u'));
        assert_eq!(e.lines[0], "abcd");
    }
    #[test]
    fn visual_block_and_replace() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["abcd".into(), "ABCD".into()];
        e.col = 1;
        e.handle(Key::Ctrl('v'));
        e.handle(Key::Char('j'));
        e.handle(Key::Char('l'));
        e.handle(Key::Char('d'));
        assert_eq!(e.lines, ["ad", "AD"]);
        e.handle(Key::Char('u'));
        assert_eq!(e.lines, ["abcd", "ABCD"]);
        e.row = 0;
        e.col = 1;
        e.handle(Key::Char('r'));
        e.handle(Key::Char('x'));
        assert_eq!(e.lines[0], "axcd");
        assert_eq!(e.mode, Mode::Normal);
    }
    #[test]
    fn cross_line_operator_and_multiline_paste() {
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["abc".into(), "def".into(), "ghi".into()];
        e.row = 0;
        e.col = 2;
        for c in "dw".chars() {
            e.handle(Key::Char(c))
        }
        assert_eq!(e.lines, ["abdef", "ghi"]);
        e.handle(Key::Char('u'));
        assert_eq!(e.lines, ["abc", "def", "ghi"]);
        e.row = 0;
        e.col = 1;
        e.handle(Key::Char('v'));
        e.handle(Key::Char('j'));
        e.handle(Key::Char('y'));
        assert_eq!(e.register.len(), 2);
        e.row = 2;
        e.col = 0;
        e.handle(Key::Char('p'));
        assert_eq!(e.lines.len(), 4);
    }
    #[test]
    fn operator_pending_mode_and_edit_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("other.txt");
        fs::write(&file, "other\n").unwrap();
        let mut e = Editor::open(None, false).unwrap();
        e.lines = vec!["current".into()];
        e.handle(Key::Char('d'));
        assert_eq!(e.mode, Mode::OperatorPending);
        e.handle(Key::Esc);
        assert_eq!(e.mode, Mode::Normal);
        e.handle(Key::Char(':'));
        for c in format!("e {}", file.display()).chars() {
            e.handle(Key::Char(c))
        }
        e.handle(Key::Enter);
        assert_eq!(e.lines[0], "other");
    }
    #[test]
    fn git_panels_show_changes_and_allow_staging() {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        let path = dir.path().join("changed.txt");
        fs::write(&path, "one\n").unwrap();
        let mut e = Editor::open(Some(path.clone()), false).unwrap();
        e.handle(Key::Char(':'));
        for c in "GitStatus".chars() {
            e.handle(Key::Char(c))
        }
        e.handle(Key::Enter);
        assert_eq!(e.mode, Mode::GitStatus);
        assert_eq!(e.git_entries.len(), 1);
        e.handle(Key::Char('s'));
        assert_eq!(e.git_entries[0].status, "A ");
        e.handle(Key::Char('q'));
        assert_eq!(e.mode, Mode::Normal);
    }
    #[test]
    fn datetime_completion_and_home_end_keys() {
        let mut editor = Editor::open(None, false).unwrap();
        editor.handle(Key::Char('i'));
        for ch in "datetime".chars() {
            editor.handle(Key::Char(ch));
        }
        assert_eq!(editor.mode, Mode::Completion);
        assert!(editor.completion_items[0].contains(' '));
        editor.handle(Key::Enter);
        assert_eq!(editor.mode, Mode::Insert);
        assert!(editor.lines[0].starts_with("20"));
        editor.handle(Key::Home);
        assert_eq!(editor.col, 0);
        editor.handle(Key::End);
        assert_eq!(editor.col, editor.lines[0].chars().count());
    }
}
