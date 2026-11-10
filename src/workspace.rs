//! Multiple open buffers with independent undo and save state.
use crate::commands::CommandHint;
use crate::lsp::TextEdit;
use crate::{Editor, Key, Mode};
use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitAxis {
    Vertical,
    Horizontal,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pane {
    pub window: usize,
    pub buffer: usize,
    pub cursor: (usize, usize),
    pub rect: Rect,
    pub active: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Divider {
    pub axis: SplitAxis,
    pub rect: Rect,
}
#[derive(Debug, Clone, Copy)]
struct Window {
    buffer: usize,
    cursor: (usize, usize),
}
enum LayoutNode {
    Leaf(usize),
    Split {
        axis: SplitAxis,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}
impl LayoutNode {
    fn window_ids(&self, out: &mut Vec<usize>) {
        match self {
            Self::Leaf(id) => out.push(*id),
            Self::Split { first, second, .. } => {
                first.window_ids(out);
                second.window_ids(out);
            }
        }
    }
    fn replace_leaf(&mut self, target: usize, axis: SplitAxis, new: usize) -> bool {
        match self {
            Self::Leaf(id) if *id == target => {
                *self = Self::Split {
                    axis,
                    first: Box::new(Self::Leaf(target)),
                    second: Box::new(Self::Leaf(new)),
                };
                true
            }
            Self::Leaf(_) => false,
            Self::Split { first, second, .. } => {
                first.replace_leaf(target, axis, new) || second.replace_leaf(target, axis, new)
            }
        }
    }
    fn remove_leaf(self, target: usize) -> Option<Self> {
        match self {
            Self::Leaf(id) => (id != target).then_some(Self::Leaf(id)),
            Self::Split {
                axis,
                first,
                second,
            } => match (first.remove_leaf(target), second.remove_leaf(target)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    axis,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(node), None) | (None, Some(node)) => Some(node),
                (None, None) => None,
            },
        }
    }
    fn collect(
        &self,
        rect: Rect,
        windows: &BTreeMap<usize, Window>,
        active: usize,
        active_cursor: (usize, usize),
        panes: &mut Vec<Pane>,
        dividers: &mut Vec<Divider>,
    ) {
        match self {
            Self::Leaf(id) => {
                if let Some(window) = windows.get(id) {
                    panes.push(Pane {
                        window: *id,
                        buffer: window.buffer,
                        cursor: if *id == active {
                            active_cursor
                        } else {
                            window.cursor
                        },
                        rect,
                        active: *id == active,
                    });
                }
            }
            Self::Split {
                axis,
                first,
                second,
            } => {
                let (one, separator, two) = match axis {
                    SplitAxis::Vertical => {
                        let one_width = rect.width.saturating_sub(1) / 2;
                        (
                            Rect {
                                width: one_width,
                                ..rect
                            },
                            Rect {
                                x: rect.x + one_width,
                                width: usize::from(rect.width > 0),
                                height: rect.height,
                                ..rect
                            },
                            Rect {
                                x: rect.x + one_width + usize::from(rect.width > 0),
                                width: rect.width.saturating_sub(one_width + 1),
                                ..rect
                            },
                        )
                    }
                    SplitAxis::Horizontal => {
                        let one_height = rect.height.saturating_sub(1) / 2;
                        (
                            Rect {
                                height: one_height,
                                ..rect
                            },
                            Rect {
                                y: rect.y + one_height,
                                height: usize::from(rect.height > 0),
                                width: rect.width,
                                ..rect
                            },
                            Rect {
                                y: rect.y + one_height + usize::from(rect.height > 0),
                                height: rect.height.saturating_sub(one_height + 1),
                                ..rect
                            },
                        )
                    }
                };
                dividers.push(Divider {
                    axis: *axis,
                    rect: separator,
                });
                first.collect(one, windows, active, active_cursor, panes, dividers);
                second.collect(two, windows, active, active_cursor, panes, dividers);
            }
        }
    }
}

pub struct Workspace {
    buffers: Vec<Editor>,
    current: usize,
    windows: BTreeMap<usize, Window>,
    tabs: Vec<LayoutNode>,
    active_tab: usize,
    active_window: usize,
    next_window: usize,
    window_prefix: bool,
    tab_prefix: bool,
    pub quit: bool,
}
impl Workspace {
    pub fn configure_options(
        &mut self,
        ignorecase: bool,
        smartcase: bool,
        autoindent: bool,
        shiftwidth: usize,
    ) {
        for buffer in &mut self.buffers {
            buffer.ignorecase = ignorecase;
            buffer.smartcase = smartcase;
            buffer.autoindent = autoindent;
            buffer.shiftwidth = shiftwidth;
        }
    }
    pub fn open(path: Option<PathBuf>, readonly: bool) -> io::Result<Self> {
        Ok(Self {
            buffers: vec![Editor::open(path, readonly)?],
            current: 0,
            windows: BTreeMap::from([(
                0,
                Window {
                    buffer: 0,
                    cursor: (0, 0),
                },
            )]),
            tabs: vec![LayoutNode::Leaf(0)],
            active_tab: 0,
            active_window: 0,
            next_window: 1,
            window_prefix: false,
            tab_prefix: false,
            quit: false,
        })
    }
    pub fn active(&self) -> &Editor {
        &self.buffers[self.current]
    }
    pub fn command_hint(&self) -> Option<CommandHint> {
        if self.active().mode != Mode::Normal {
            return self.active().command_hint();
        }
        if self.window_prefix {
            return Some(CommandHint {
                title: "Finestre",
                choices: &[
                    ("h/j/k/l", "sposta focus"),
                    ("w", "prossima"),
                    ("s", "dividi orizz."),
                    ("v", "dividi vert."),
                    ("c", "chiudi"),
                ],
            });
        }
        if self.tab_prefix {
            return Some(CommandHint {
                title: "Navigazione g",
                choices: &[
                    ("g", "prima riga"),
                    ("d", "definizione"),
                    ("r", "riferimenti"),
                    ("t/T", "schede"),
                ],
            });
        }
        self.active().command_hint()
    }
    pub fn active_mut(&mut self) -> &mut Editor {
        &mut self.buffers[self.current]
    }
    pub fn buffer(&self, index: usize) -> &Editor {
        &self.buffers[index]
    }
    pub fn window_count(&self) -> usize {
        let mut ids = Vec::new();
        self.tabs[self.active_tab].window_ids(&mut ids);
        ids.len()
    }
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }
    pub fn active_tab_index(&self) -> usize {
        self.active_tab
    }
    pub fn tab_titles(&self) -> Vec<(String, bool)> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let mut ids = Vec::new();
                tab.window_ids(&mut ids);
                let name = ids
                    .first()
                    .and_then(|id| self.windows.get(id))
                    .and_then(|window| self.buffers.get(window.buffer))
                    .and_then(|buffer| buffer.path.as_ref())
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "[No Name]".into());
                (name, index == self.active_tab)
            })
            .collect()
    }
    pub fn panes(&self, rect: Rect) -> (Vec<Pane>, Vec<Divider>) {
        let mut panes = Vec::new();
        let mut dividers = Vec::new();
        self.tabs[self.active_tab].collect(
            rect,
            &self.windows,
            self.active_window,
            (self.active().row, self.active().col),
            &mut panes,
            &mut dividers,
        );
        (panes, dividers)
    }
    fn set_active_buffer(&mut self, index: usize) {
        self.current = index;
        let cursor = (self.active().row, self.active().col);
        if let Some(window) = self.windows.get_mut(&self.active_window) {
            window.buffer = index;
            window.cursor = cursor;
        }
    }
    fn switch_window(&mut self, target: usize) {
        if target == self.active_window || !self.windows.contains_key(&target) {
            return;
        }
        let cursor = (self.active().row, self.active().col);
        if let Some(window) = self.windows.get_mut(&self.active_window) {
            window.cursor = cursor;
        }
        self.active_window = target;
        let target = self.windows[&target];
        self.current = target.buffer;
        let editor = self.active_mut();
        editor.row = target.cursor.0.min(editor.lines.len().saturating_sub(1));
        editor.col = target
            .cursor
            .1
            .min(editor.lines[editor.row].chars().count().saturating_sub(1));
    }
    fn split(&mut self, axis: SplitAxis, path: Option<PathBuf>) -> io::Result<()> {
        let buffer = if let Some(path) = path {
            if let Some(index) = self
                .buffers
                .iter()
                .position(|buffer| buffer.path.as_ref() == Some(&path))
            {
                index
            } else {
                self.buffers
                    .push(Editor::open(Some(path), self.active().readonly)?);
                self.buffers.len() - 1
            }
        } else {
            self.current
        };
        let new = self.next_window;
        self.next_window += 1;
        let cursor = if buffer == self.current {
            (self.active().row, self.active().col)
        } else {
            (self.buffers[buffer].row, self.buffers[buffer].col)
        };
        self.windows.insert(new, Window { buffer, cursor });
        self.tabs[self.active_tab].replace_leaf(self.active_window, axis, new);
        self.switch_window(new);
        Ok(())
    }
    fn close_window(&mut self) {
        if self.window_count() == 1 {
            if self.tabs.len() > 1 {
                self.close_tab();
            } else {
                if self.buffers.iter().any(|buffer| buffer.modified) {
                    self.active_mut().message = "Unsaved buffers (:q! to quit)".into();
                } else {
                    self.quit = true;
                }
            }
            return;
        }
        let old = self.active_window;
        let layout = std::mem::replace(&mut self.tabs[self.active_tab], LayoutNode::Leaf(old));
        self.tabs[self.active_tab] = layout.remove_leaf(old).expect("another window exists");
        self.windows.remove(&old);
        let mut ids = Vec::new();
        self.tabs[self.active_tab].window_ids(&mut ids);
        let next = ids[0];
        self.active_window = old;
        self.switch_window(next);
    }
    fn new_tab(&mut self, path: Option<PathBuf>) -> io::Result<()> {
        let buffer = if let Some(path) = path {
            if let Some(index) = self
                .buffers
                .iter()
                .position(|buffer| buffer.path.as_ref() == Some(&path))
            {
                index
            } else {
                self.buffers
                    .push(Editor::open(Some(path), self.active().readonly)?);
                self.buffers.len() - 1
            }
        } else {
            self.current
        };
        let new = self.next_window;
        self.next_window += 1;
        self.windows.insert(
            new,
            Window {
                buffer,
                cursor: (self.buffers[buffer].row, self.buffers[buffer].col),
            },
        );
        self.tabs.push(LayoutNode::Leaf(new));
        self.active_tab = self.tabs.len() - 1;
        self.switch_window(new);
        Ok(())
    }
    fn switch_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.active_tab {
            return;
        }
        self.active_tab = index;
        let mut ids = Vec::new();
        self.tabs[index].window_ids(&mut ids);
        self.switch_window(ids[0]);
    }
    fn close_tab(&mut self) {
        if self.tabs.len() == 1 {
            if self.buffers.iter().any(|buffer| buffer.modified) {
                self.active_mut().message = "Unsaved buffers (:tabclose! to quit)".into();
            } else {
                self.quit = true;
            }
            return;
        }
        let mut ids = Vec::new();
        self.tabs[self.active_tab].window_ids(&mut ids);
        for id in ids {
            self.windows.remove(&id);
        }
        self.tabs.remove(self.active_tab);
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        let mut next = Vec::new();
        self.tabs[self.active_tab].window_ids(&mut next);
        self.switch_window(next[0]);
    }
    fn navigate_window(&mut self, key: char) {
        let (panes, _) = self.panes(Rect {
            x: 0,
            y: 0,
            width: 1000,
            height: 1000,
        });
        let Some(current) = panes.iter().find(|pane| pane.active) else {
            return;
        };
        let target = if key == 'w' {
            panes
                .iter()
                .position(|pane| pane.active)
                .and_then(|index| panes.get((index + 1) % panes.len()))
        } else {
            let center = |pane: &Pane| {
                (
                    pane.rect.x + pane.rect.width / 2,
                    pane.rect.y + pane.rect.height / 2,
                )
            };
            let (x, y) = center(current);
            panes
                .iter()
                .filter(|pane| !pane.active)
                .filter_map(|pane| {
                    let (other_x, other_y) = center(pane);
                    let valid = match key {
                        'h' => other_x < x,
                        'l' => other_x > x,
                        'k' => other_y < y,
                        'j' => other_y > y,
                        _ => false,
                    };
                    valid.then_some((pane, x.abs_diff(other_x) + y.abs_diff(other_y)))
                })
                .min_by_key(|(_, distance)| *distance)
                .map(|(pane, _)| pane)
        };
        if let Some(target) = target {
            self.switch_window(target.window);
        }
    }
    pub fn len(&self) -> usize {
        self.buffers.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }
    pub fn open_location(&mut self, path: PathBuf, row: usize, col: usize) -> io::Result<()> {
        let index = if let Some(index) = self
            .buffers
            .iter()
            .position(|buffer| buffer.path.as_ref() == Some(&path))
        {
            index
        } else {
            self.buffers
                .push(Editor::open(Some(path), self.active().readonly)?);
            self.buffers.len() - 1
        };
        self.set_active_buffer(index);
        let cursor = {
            let editor = self.active_mut();
            editor.row = row.min(editor.lines.len().saturating_sub(1));
            editor.col = col.min(editor.lines[editor.row].chars().count().saturating_sub(1));
            (editor.row, editor.col)
        };
        if let Some(window) = self.windows.get_mut(&self.active_window) {
            window.cursor = cursor;
        }
        Ok(())
    }
    pub fn apply_workspace_edits(
        &mut self,
        changes: Vec<(PathBuf, Vec<TextEdit>)>,
    ) -> io::Result<usize> {
        let mut staged = Vec::new();
        let mut total = 0;
        for (path, edits) in changes {
            if !path.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("Rename target unavailable: {}", path.display()),
                ));
            }
            let index = self
                .buffers
                .iter()
                .position(|buffer| buffer.path.as_ref() == Some(&path));
            let mut editor = if let Some(index) = index {
                self.buffers[index].clone()
            } else {
                Editor::open(Some(path), self.active().readonly)?
            };
            total += editor.apply_lsp_edits(&edits)?;
            staged.push((index, editor));
        }
        for (index, editor) in staged {
            if let Some(index) = index {
                self.buffers[index] = editor;
            } else {
                self.buffers.push(editor);
            }
        }
        Ok(total)
    }
    pub fn tabs(&self) -> Vec<(String, bool, bool)> {
        self.buffers
            .iter()
            .enumerate()
            .map(|(index, editor)| {
                (
                    editor
                        .path
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| {
                            if editor.mode == Mode::Explorer {
                                "[Explorer]".into()
                            } else {
                                "[No Name]".into()
                            }
                        }),
                    editor.modified,
                    index == self.current,
                )
            })
            .collect()
    }
    pub fn current_index(&self) -> usize {
        self.current
    }
    fn close(&mut self, force: bool) {
        if self.active().modified && !force {
            self.active_mut().message = "No write since last change (:w or :bd!)".into();
            return;
        }
        if self.buffers.len() == 1 {
            if self.windows.len() > 1 {
                self.buffers[0] =
                    Editor::open(None, self.active().readonly).expect("empty buffer can open");
                for window in self.windows.values_mut() {
                    window.cursor = (0, 0);
                }
            } else {
                self.quit = true
            }
        } else {
            let removed = self.current;
            self.buffers.remove(removed);
            for window in self.windows.values_mut() {
                if window.buffer == removed {
                    window.buffer = removed.min(self.buffers.len() - 1);
                    window.cursor = (0, 0);
                } else if window.buffer > removed {
                    window.buffer -= 1;
                }
            }
            self.current = self.windows[&self.active_window].buffer;
        }
    }
    fn write_all(&mut self) -> io::Result<usize> {
        let mut written = 0;
        for buffer in &mut self.buffers {
            if buffer.modified {
                buffer.save()?;
                written += 1;
            }
        }
        Ok(written)
    }
    fn command(&mut self, command: &str) -> bool {
        match command {
            "wa" | "wall" | "wqa" | "wqall" => {
                match self.write_all() {
                    Ok(written) => {
                        if matches!(command, "wqa" | "wqall") {
                            self.quit = true;
                        } else {
                            self.active_mut().message = format!("{written} buffers written");
                        }
                    }
                    Err(e) => self.active_mut().message = format!("Write all failed: {e}"),
                }
                true
            }
            "bn" | "bnext" => {
                self.set_active_buffer((self.current + 1) % self.buffers.len());
                true
            }
            "bp" | "bprevious" => {
                self.set_active_buffer(
                    (self.current + self.buffers.len() - 1) % self.buffers.len(),
                );
                true
            }
            "tabnew" | "tabedit" => {
                if let Err(e) = self.new_tab(None) {
                    self.active_mut().message = e.to_string();
                }
                true
            }
            _ if command.starts_with("tabnew ") || command.starts_with("tabedit ") => {
                let path = PathBuf::from(command.split_once(' ').unwrap().1.trim());
                if let Err(e) = self.new_tab(Some(path)) {
                    self.active_mut().message = e.to_string();
                }
                true
            }
            "tabnext" | "tabn" | "gt" => {
                self.switch_tab((self.active_tab + 1) % self.tabs.len());
                true
            }
            "tabprevious" | "tabp" | "gT" => {
                self.switch_tab((self.active_tab + self.tabs.len() - 1) % self.tabs.len());
                true
            }
            "tabclose" | "tabc" => {
                self.close_tab();
                true
            }
            "tabclose!" | "tabc!" => {
                if self.tabs.len() == 1 {
                    self.quit = true;
                } else {
                    self.close_tab();
                }
                true
            }
            "split" | "sp" => {
                if let Err(e) = self.split(SplitAxis::Horizontal, None) {
                    self.active_mut().message = e.to_string();
                }
                true
            }
            "vsplit" | "vs" => {
                if let Err(e) = self.split(SplitAxis::Vertical, None) {
                    self.active_mut().message = e.to_string();
                }
                true
            }
            _ if command.starts_with("split ") || command.starts_with("sp ") => {
                let path = PathBuf::from(command.split_once(' ').unwrap().1.trim());
                if let Err(e) = self.split(SplitAxis::Horizontal, Some(path)) {
                    self.active_mut().message = e.to_string();
                }
                true
            }
            _ if command.starts_with("vsplit ") || command.starts_with("vs ") => {
                let path = PathBuf::from(command.split_once(' ').unwrap().1.trim());
                if let Err(e) = self.split(SplitAxis::Vertical, Some(path)) {
                    self.active_mut().message = e.to_string();
                }
                true
            }
            "buffers" | "ls" => {
                self.active_mut().message = self
                    .buffers
                    .iter()
                    .enumerate()
                    .map(|(i, b)| {
                        format!(
                            "{}{}:{}{}",
                            i + 1,
                            if i == self.current { '*' } else { ' ' },
                            b.path
                                .as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|| "[No Name]".into()),
                            if b.modified { "+" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("  ");
                true
            }
            "bd" | "bdelete" => {
                self.close(false);
                true
            }
            "bd!" | "bdelete!" => {
                self.close(true);
                true
            }
            "q" | "quit" if self.window_count() > 1 || self.tabs.len() > 1 => {
                self.close_window();
                true
            }
            "q!" | "quit!" if self.window_count() > 1 || self.tabs.len() > 1 => {
                self.close_window();
                true
            }
            "wq" | "x" if self.window_count() > 1 || self.tabs.len() > 1 => {
                match self.active_mut().save() {
                    Ok(()) => self.close_window(),
                    Err(e) => self.active_mut().message = e.to_string(),
                }
                true
            }
            "q" | "quit" => {
                if self.buffers.iter().any(|buffer| buffer.modified) {
                    self.active_mut().message = "Unsaved buffers (:q! to quit)".into();
                } else {
                    self.quit = true;
                }
                true
            }
            "q!" | "quit!" => {
                self.quit = true;
                true
            }
            "wq" | "x" => {
                match self.active_mut().save() {
                    Ok(()) => {
                        if self.buffers.iter().any(|buffer| buffer.modified) {
                            self.active_mut().message = "Unsaved buffers (:q! to quit)".into();
                        } else {
                            self.quit = true;
                        }
                    }
                    Err(e) => self.active_mut().message = e.to_string(),
                }
                true
            }
            _ if command.starts_with("e ") || command.starts_with("edit ") => {
                let name = command
                    .split_once(' ')
                    .map(|(_, value)| value.trim())
                    .unwrap_or("");
                if name.is_empty() {
                    self.active_mut().message = "File name required".into()
                } else {
                    let path = PathBuf::from(name);
                    if let Some(index) = self
                        .buffers
                        .iter()
                        .position(|b| b.path.as_ref() == Some(&path))
                    {
                        self.set_active_buffer(index)
                    } else {
                        match Editor::open(Some(path), self.active().readonly) {
                            Ok(editor) => {
                                self.buffers.push(editor);
                                self.set_active_buffer(self.buffers.len() - 1)
                            }
                            Err(e) => self.active_mut().message = e.to_string(),
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }
    pub fn handle(&mut self, key: Key) {
        if self.active().mode == Mode::Normal {
            if self.tab_prefix {
                self.tab_prefix = false;
                match key {
                    Key::Char('t') => {
                        self.switch_tab((self.active_tab + 1) % self.tabs.len());
                        return;
                    }
                    Key::Char('T') => {
                        self.switch_tab((self.active_tab + self.tabs.len() - 1) % self.tabs.len());
                        return;
                    }
                    _ => self.active_mut().handle(Key::Char('g')),
                }
            } else if key == Key::Char('g') {
                self.tab_prefix = true;
                return;
            }
            if self.window_prefix {
                self.window_prefix = false;
                match key {
                    Key::Char('h' | 'j' | 'k' | 'l' | 'w') => {
                        if let Key::Char(direction) = key {
                            self.navigate_window(direction);
                        }
                    }
                    Key::Char('s') => {
                        if let Err(e) = self.split(SplitAxis::Horizontal, None) {
                            self.active_mut().message = e.to_string();
                        }
                    }
                    Key::Char('v') => {
                        if let Err(e) = self.split(SplitAxis::Vertical, None) {
                            self.active_mut().message = e.to_string();
                        }
                    }
                    Key::Char('c') => self.close_window(),
                    _ => (),
                }
                return;
            }
            if key == Key::Ctrl('w') {
                self.window_prefix = true;
                return;
            }
        }
        if self.active().mode == Mode::Command && key == Key::Enter {
            let command = self.active().prompt.trim().to_owned();
            let previous = self.current;
            if self.command(&command) {
                if let Some(buffer) = self.buffers.get_mut(previous) {
                    if buffer.mode == Mode::Command {
                        buffer.mode = Mode::Normal;
                        buffer.prompt.clear()
                    }
                }
                return;
            }
        }
        let previous = if key == Key::Enter
            && matches!(
                self.active().mode,
                Mode::Explorer | Mode::Picker | Mode::Quickfix | Mode::GitStatus
            ) {
            Some(self.active().before_navigation())
        } else {
            None
        };
        self.active_mut().handle(key);
        if let Some(old) = previous {
            if self.active().path != old.path && self.active().path.is_some() {
                let replaced = self.current;
                self.buffers.insert(replaced, old);
                for (id, window) in &mut self.windows {
                    if window.buffer > replaced {
                        window.buffer += 1;
                    }
                    if *id == self.active_window {
                        window.buffer = replaced + 1;
                    }
                }
                self.current += 1;
            }
        }
        if self.active().quit {
            self.quit = true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tabs_keep_separate_layouts_and_share_buffers() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("one.txt");
        let second = dir.path().join("two.txt");
        std::fs::write(&first, "one\n").unwrap();
        std::fs::write(&second, "two\n").unwrap();
        let mut workspace = Workspace::open(Some(first.clone()), false).unwrap();
        ex(&mut workspace, "vsplit");
        assert_eq!(workspace.window_count(), 2);
        ex(&mut workspace, &format!("tabnew {}", second.display()));
        assert_eq!(workspace.tab_count(), 2);
        assert_eq!(workspace.window_count(), 1);
        assert_eq!(workspace.active().path.as_ref(), Some(&second));
        workspace.handle(Key::Char('g'));
        workspace.handle(Key::Char('T'));
        assert_eq!(workspace.window_count(), 2);
        assert_eq!(workspace.active().path.as_ref(), Some(&first));
        workspace.handle(Key::Char('g'));
        workspace.handle(Key::Char('t'));
        assert_eq!(workspace.active().path.as_ref(), Some(&second));
        ex(&mut workspace, "tabclose");
        assert_eq!(workspace.tab_count(), 1);
        assert_eq!(workspace.window_count(), 2);
        assert!(!workspace.quit);
    }
    #[test]
    fn last_tab_close_preserves_unsaved_buffer() {
        let mut workspace = Workspace::open(None, false).unwrap();
        workspace.handle(Key::Char('i'));
        workspace.handle(Key::Char('x'));
        workspace.handle(Key::Esc);
        ex(&mut workspace, "tabclose");
        assert!(!workspace.quit);
        assert_eq!(workspace.active().lines[0], "x");
        ex(&mut workspace, "tabclose!");
        assert!(workspace.quit);
    }
    #[test]
    fn close_last_window_preserves_unsaved_buffer() {
        let mut workspace = Workspace::open(None, false).unwrap();
        workspace.handle(Key::Char('i'));
        workspace.handle(Key::Char('x'));
        workspace.handle(Key::Esc);
        workspace.handle(Key::Ctrl('w'));
        workspace.handle(Key::Char('c'));
        assert!(!workspace.quit);
        assert_eq!(workspace.active().lines[0], "x");
    }
    #[test]
    fn normal_g_prefix_still_reaches_vim_parser() {
        let mut workspace = Workspace::open(None, false).unwrap();
        workspace.active_mut().lines = vec!["first".into(), "second".into()];
        workspace.handle(Key::Char('j'));
        workspace.handle(Key::Char('g'));
        workspace.handle(Key::Char('g'));
        assert_eq!(workspace.active().row, 0);
    }
    #[test]
    fn workspace_rename_stages_all_files_before_mutating() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one.rs");
        let two = dir.path().join("two.rs");
        std::fs::write(&one, "fn old() {}\n").unwrap();
        std::fs::write(&two, "old();\n").unwrap();
        let mut workspace = Workspace::open(Some(one.clone()), false).unwrap();
        let valid = TextEdit {
            start: (0, 3),
            end: (0, 6),
            new_text: "new".into(),
        };
        let bad = TextEdit {
            start: (9, 0),
            end: (9, 3),
            new_text: "new".into(),
        };
        assert!(workspace
            .apply_workspace_edits(vec![
                (one.clone(), vec![valid.clone()]),
                (two.clone(), vec![bad])
            ])
            .is_err());
        assert_eq!(workspace.active().lines[0], "fn old() {}");
        assert_eq!(workspace.len(), 1);
        let other = TextEdit {
            start: (0, 0),
            end: (0, 3),
            new_text: "new".into(),
        };
        assert_eq!(
            workspace
                .apply_workspace_edits(vec![(one, vec![valid]), (two, vec![other])])
                .unwrap(),
            2
        );
        assert_eq!(workspace.active().lines[0], "fn new() {}");
        assert_eq!(workspace.len(), 2);
        assert_eq!(workspace.buffer(1).lines[0], "new();");
        assert!(workspace.buffer(1).modified);
        ex(&mut workspace, "wa");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("one.rs")).unwrap(),
            "fn new() {}\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("two.rs")).unwrap(),
            "new();\n"
        );
        assert!(!workspace.buffer(1).modified);
    }
    #[test]
    fn vertical_split_shares_buffer_and_keeps_window_cursors() {
        let mut workspace = Workspace::open(None, false).unwrap();
        workspace.active_mut().lines = vec!["alpha".into(), "beta".into()];
        ex(&mut workspace, "vsplit");
        assert_eq!(workspace.window_count(), 2);
        let (panes, dividers) = workspace.panes(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        });
        assert_eq!(panes.len(), 2);
        assert_eq!(dividers[0].axis, SplitAxis::Vertical);
        workspace.handle(Key::Char('j'));
        workspace.handle(Key::Char('A'));
        workspace.handle(Key::Char('!'));
        workspace.handle(Key::Esc);
        assert_eq!(workspace.active().lines[1], "beta!");
        workspace.handle(Key::Ctrl('w'));
        workspace.handle(Key::Char('h'));
        assert_eq!((workspace.active().row, workspace.active().col), (0, 0));
        assert_eq!(workspace.active().lines[1], "beta!");
        workspace.handle(Key::Ctrl('w'));
        workspace.handle(Key::Char('l'));
        assert_eq!(workspace.active().row, 1);
        ex(&mut workspace, "q");
        assert_eq!(workspace.window_count(), 1);
        assert!(!workspace.quit);
        assert_eq!(workspace.active().lines[1], "beta!");
    }
    #[test]
    fn horizontal_split_opens_second_file_without_losing_first() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one.txt");
        let two = dir.path().join("two.txt");
        std::fs::write(&one, "one\n").unwrap();
        std::fs::write(&two, "two\n").unwrap();
        let mut workspace = Workspace::open(Some(one.clone()), false).unwrap();
        ex(&mut workspace, &format!("split {}", two.display()));
        assert_eq!(workspace.window_count(), 2);
        assert_eq!(workspace.len(), 2);
        assert_eq!(workspace.active().path.as_ref(), Some(&two));
        let (_, dividers) = workspace.panes(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        });
        assert_eq!(dividers[0].axis, SplitAxis::Horizontal);
        workspace.handle(Key::Ctrl('w'));
        workspace.handle(Key::Char('k'));
        assert_eq!(workspace.active().path.as_ref(), Some(&one));
    }
    #[test]
    fn quit_checks_hidden_modified_buffers() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one.txt");
        let two = dir.path().join("two.txt");
        std::fs::write(&one, "one\n").unwrap();
        std::fs::write(&two, "two\n").unwrap();
        let mut workspace = Workspace::open(Some(one), false).unwrap();
        workspace.handle(Key::Char('i'));
        workspace.handle(Key::Char('X'));
        workspace.handle(Key::Esc);
        ex(&mut workspace, &format!("e {}", two.display()));
        ex(&mut workspace, "q");
        assert!(!workspace.quit);
        assert!(workspace.active().message.contains("Unsaved buffers"));
        ex(&mut workspace, "q!");
        assert!(workspace.quit);
    }
    #[test]
    fn picker_open_in_split_keeps_previous_pane_on_old_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one.txt");
        let two = dir.path().join("two.txt");
        std::fs::write(&one, "one\n").unwrap();
        std::fs::write(&two, "two\n").unwrap();
        let mut workspace = Workspace::open(Some(one.clone()), false).unwrap();
        ex(&mut workspace, "vsplit");
        workspace.handle(Key::Ctrl('p'));
        assert_eq!(workspace.active().mode, Mode::Picker);
        for c in "two".chars() {
            workspace.handle(Key::Char(c));
        }
        assert_eq!(workspace.active().picker_results, vec![two.clone()]);
        workspace.handle(Key::Enter);
        assert_eq!(workspace.active().path.as_ref(), Some(&two));
        workspace.handle(Key::Ctrl('w'));
        workspace.handle(Key::Char('h'));
        assert_eq!(workspace.active().path.as_ref(), Some(&one));
    }
    fn ex(workspace: &mut Workspace, cmd: &str) {
        workspace.handle(Key::Char(':'));
        for c in cmd.chars() {
            workspace.handle(Key::Char(c))
        }
        workspace.handle(Key::Enter);
    }
    #[test]
    fn switch_buffers_preserves_unsaved_state_and_undo() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("one.txt");
        let second = dir.path().join("two.txt");
        std::fs::write(&first, "one\n").unwrap();
        std::fs::write(&second, "two\n").unwrap();
        let mut workspace = Workspace::open(Some(first.clone()), false).unwrap();
        workspace.handle(Key::Char('i'));
        workspace.handle(Key::Char('X'));
        workspace.handle(Key::Esc);
        ex(&mut workspace, &format!("e {}", second.display()));
        assert_eq!(workspace.len(), 2);
        assert_eq!(workspace.active().lines[0], "two");
        ex(&mut workspace, "bp");
        assert_eq!(workspace.active().lines[0], "Xone");
        workspace.handle(Key::Char('u'));
        assert_eq!(workspace.active().lines[0], "one");
        ex(&mut workspace, "bn");
        assert_eq!(workspace.active().path.as_ref(), Some(&second));
    }
    #[test]
    fn closing_modified_buffer_requires_force() {
        let mut workspace = Workspace::open(None, false).unwrap();
        workspace.handle(Key::Char('i'));
        workspace.handle(Key::Char('x'));
        workspace.handle(Key::Esc);
        ex(&mut workspace, "bd");
        assert!(!workspace.quit);
        ex(&mut workspace, "bd!");
        assert!(workspace.quit);
    }
    #[test]
    fn picker_keeps_previous_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let mut workspace = Workspace::open(Some(dir.path().to_path_buf()), false).unwrap();
        workspace.handle(Key::Ctrl('p'));
        workspace.handle(Key::Enter);
        assert_eq!(workspace.len(), 2);
        ex(&mut workspace, "bp");
        assert_eq!(workspace.active().mode, Mode::Explorer);
    }
}
