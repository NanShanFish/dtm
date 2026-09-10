use crate::pack::is_ignored_directory;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, queue};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionState {
    None,
    Partial,
    All,
}

#[derive(Debug)]
struct TreeNode {
    path: PathBuf,
    depth: usize,
    parent: Option<usize>,
    children: Vec<usize>,
    is_directory: bool,
    expanded: bool,
    selected: bool,
}

#[derive(Debug)]
struct TreeSelection {
    nodes: Vec<TreeNode>,
    cursor: usize,
    excluded_root: Option<PathBuf>,
}

impl TreeSelection {
    fn load_excluding(root: &Path, excluded_root: Option<&Path>) -> Result<Self, InteractiveError> {
        let root = fs::canonicalize(root).map_err(|source| InteractiveError::ReadDirectory {
            path: root.to_path_buf(),
            source,
        })?;
        let metadata =
            fs::symlink_metadata(&root).map_err(|source| InteractiveError::ReadDirectory {
                path: root.clone(),
                source,
            })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(InteractiveError::NotDirectory(root));
        }
        let excluded_root = excluded_root.map(Path::to_path_buf);
        if excluded_root
            .as_ref()
            .is_some_and(|excluded| root.starts_with(excluded))
        {
            return Err(InteractiveError::ExcludedRoot(root));
        }
        let mut selection = Self {
            nodes: vec![TreeNode {
                path: root,
                depth: 0,
                parent: None,
                children: Vec::new(),
                is_directory: true,
                expanded: true,
                selected: false,
            }],
            cursor: 0,
            excluded_root,
        };
        selection.scan_directory(0)?;
        Ok(selection)
    }

    fn scan_directory(&mut self, parent: usize) -> Result<(), InteractiveError> {
        let directory = self.nodes[parent].path.clone();
        let mut entries = fs::read_dir(&directory)
            .map_err(|source| InteractiveError::ReadDirectory {
                path: directory.clone(),
                source,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| InteractiveError::ReadDirectory {
                path: directory.clone(),
                source,
            })?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            if self
                .excluded_root
                .as_ref()
                .is_some_and(|excluded| path.starts_with(excluded))
            {
                continue;
            }
            let file_type =
                entry
                    .file_type()
                    .map_err(|source| InteractiveError::ReadDirectory {
                        path: path.clone(),
                        source,
                    })?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() && is_ignored_directory(&path) {
                continue;
            }
            if !file_type.is_dir() && !file_type.is_file() {
                continue;
            }
            let index = self.nodes.len();
            self.nodes.push(TreeNode {
                path,
                depth: self.nodes[parent].depth + 1,
                parent: Some(parent),
                children: Vec::new(),
                is_directory: file_type.is_dir(),
                expanded: false,
                selected: false,
            });
            self.nodes[parent].children.push(index);
            if file_type.is_dir() {
                self.scan_directory(index)?;
            }
        }
        Ok(())
    }

    fn visible(&self) -> Vec<usize> {
        let mut visible = vec![0];
        self.append_visible(0, &mut visible);
        visible
    }

    fn append_visible(&self, index: usize, visible: &mut Vec<usize>) {
        if !self.nodes[index].expanded {
            return;
        }
        for child in &self.nodes[index].children {
            visible.push(*child);
            self.append_visible(*child, visible);
        }
    }

    fn selection_state(&self, index: usize) -> SelectionState {
        let node = &self.nodes[index];
        if !node.is_directory {
            return if node.selected {
                SelectionState::All
            } else {
                SelectionState::None
            };
        }
        let leaves = self.leaves(index);
        if leaves.is_empty() || leaves.iter().all(|leaf| !self.nodes[*leaf].selected) {
            SelectionState::None
        } else if leaves.iter().all(|leaf| self.nodes[*leaf].selected) {
            SelectionState::All
        } else {
            SelectionState::Partial
        }
    }

    fn leaves(&self, index: usize) -> Vec<usize> {
        let mut leaves = Vec::new();
        self.append_leaves(index, &mut leaves);
        leaves
    }

    fn append_leaves(&self, index: usize, leaves: &mut Vec<usize>) {
        let node = &self.nodes[index];
        if !node.is_directory {
            leaves.push(index);
            return;
        }
        for child in &node.children {
            self.append_leaves(*child, leaves);
        }
    }

    fn toggle(&mut self, index: usize) {
        let selected = self.selection_state(index) != SelectionState::All;
        for leaf in self.leaves(index) {
            self.nodes[leaf].selected = selected;
        }
    }

    fn toggle_all(&mut self) {
        self.toggle(0);
    }

    fn move_cursor(&mut self, delta: isize) {
        let visible = self.visible();
        let position = visible
            .iter()
            .position(|index| *index == self.cursor)
            .unwrap_or(0);
        let next = position
            .saturating_add_signed(delta)
            .min(visible.len().saturating_sub(1));
        self.cursor = visible[next];
    }

    fn collapse_or_parent(&mut self) {
        if self.nodes[self.cursor].is_directory && self.nodes[self.cursor].expanded {
            self.nodes[self.cursor].expanded = false;
        } else if let Some(parent) = self.nodes[self.cursor].parent {
            self.cursor = parent;
        }
    }

    fn expand(&mut self) {
        if self.nodes[self.cursor].is_directory {
            self.nodes[self.cursor].expanded = true;
        }
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        self.nodes
            .iter()
            .filter(|node| !node.is_directory && node.selected)
            .map(|node| node.path.clone())
            .collect()
    }
}

pub fn select_files(root: &Path, excluded_root: &Path) -> Result<Vec<PathBuf>, InteractiveError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(InteractiveError::NotTerminal);
    }
    let mut tree = TreeSelection::load_excluding(root, Some(excluded_root))?;
    let mut terminal = TerminalGuard::enter()?;
    loop {
        terminal.draw(&tree)?;
        let Event::Key(key) = event::read().map_err(InteractiveError::Terminal)? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL)
            | (KeyCode::Esc, _)
            | (KeyCode::Char('q'), _) => {
                return Err(InteractiveError::Cancelled);
            }
            (KeyCode::Enter, _) => return Ok(tree.selected_paths()),
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => tree.move_cursor(-1),
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => tree.move_cursor(1),
            (KeyCode::Left, _) | (KeyCode::Char('h'), _) => tree.collapse_or_parent(),
            (KeyCode::Right, _) | (KeyCode::Char('l'), _) => tree.expand(),
            (KeyCode::Char(' '), _) => tree.toggle(tree.cursor),
            (KeyCode::Char('a'), _) => tree.toggle_all(),
            _ => {}
        }
    }
}

struct TerminalGuard {
    stdout: io::Stdout,
}

impl TerminalGuard {
    fn enter() -> Result<Self, InteractiveError> {
        enable_raw_mode().map_err(InteractiveError::Terminal)?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(InteractiveError::Terminal(error));
        }
        Ok(Self { stdout })
    }

    fn draw(&mut self, tree: &TreeSelection) -> Result<(), InteractiveError> {
        let (width, height) = crossterm::terminal::size().map_err(InteractiveError::Terminal)?;
        let rows = usize::from(height.saturating_sub(2));
        let visible = tree.visible();
        let cursor_position = visible
            .iter()
            .position(|index| *index == tree.cursor)
            .unwrap_or(0);
        let start = cursor_position.saturating_sub(rows.saturating_sub(1));
        queue!(self.stdout, MoveTo(0, 0), Clear(ClearType::All))
            .map_err(InteractiveError::Terminal)?;
        for index in visible.iter().skip(start).take(rows) {
            let node = &tree.nodes[*index];
            let pointer = if *index == tree.cursor { ">" } else { " " };
            let state = match tree.selection_state(*index) {
                SelectionState::None => "[ ]",
                SelectionState::Partial => "[-]",
                SelectionState::All => "[x]",
            };
            let marker = if node.is_directory {
                if node.expanded { "v" } else { ">" }
            } else {
                " "
            };
            let name = if node.depth == 0 {
                node.path.display().to_string()
            } else {
                node.path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| node.path.display().to_string())
            };
            let line = format!(
                "{pointer} {state} {}{marker} {name}",
                "  ".repeat(node.depth)
            );
            write!(
                self.stdout,
                "{}\r\n",
                truncate_line(&line, usize::from(width))
            )
            .map_err(InteractiveError::Terminal)?;
        }
        let help = "Space select  a all/none  arrows navigate  Enter confirm  q cancel";
        write!(self.stdout, "{}", truncate_line(help, usize::from(width)))
            .map_err(InteractiveError::Terminal)?;
        self.stdout.flush().map_err(InteractiveError::Terminal)
    }
}

fn truncate_line(line: &str, width: usize) -> String {
    if line.width() <= width {
        return line.to_owned();
    }
    if width <= 3 {
        return line
            .chars()
            .scan(0, |used, character| {
                let character_width = character.width().unwrap_or(0);
                if *used + character_width > width {
                    None
                } else {
                    *used += character_width;
                    Some(character)
                }
            })
            .collect();
    }
    let available = width - 3;
    let mut used = 0;
    let mut truncated = String::new();
    for character in line.chars() {
        let character_width = character.width().unwrap_or(0);
        if used + character_width > available {
            break;
        }
        used += character_width;
        truncated.push(character);
    }
    truncated.push_str("...");
    truncated
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(self.stdout, Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

#[derive(Debug)]
pub enum InteractiveError {
    NotTerminal,
    NotDirectory(PathBuf),
    ExcludedRoot(PathBuf),
    ReadDirectory { path: PathBuf, source: io::Error },
    Terminal(io::Error),
    Cancelled,
}

impl fmt::Display for InteractiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotTerminal => write!(formatter, "interactive pack requires a terminal"),
            Self::NotDirectory(path) => write!(
                formatter,
                "interactive pack root is not a directory: {}",
                path.display()
            ),
            Self::ExcludedRoot(path) => write!(
                formatter,
                "interactive pack root is inside the package directory: {}",
                path.display()
            ),
            Self::ReadDirectory { path, source } => {
                write!(formatter, "could not scan {}: {source}", path.display())
            }
            Self::Terminal(error) => write!(formatter, "interactive terminal error: {error}"),
            Self::Cancelled => write!(formatter, "interactive pack cancelled"),
        }
    }
}

impl Error for InteractiveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadDirectory { source, .. } | Self::Terminal(source) => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
