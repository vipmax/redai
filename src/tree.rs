use crate::app::Theme;
use crate::utils::{find_color, is_ignored_path};
use crossterm::event::{Event, KeyCode, KeyEventKind, MouseEventKind};
use notify::event::ModifyKind;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use std::path::Path;
use tui_tree_widget::{Tree, TreeItem, TreeState};

pub enum TreeAction {
    None,
    OpenFile(String),
    Quit,
}

pub struct TreePanel {
    pub state: TreeState<String>,
    pub items: Vec<TreeItem<'static, String>>,
}

impl TreePanel {
    pub fn new(root_path: &Path, theme: &Theme) -> Self {
        let items = build_initial_tree_items(root_path, theme);
        let mut state = TreeState::default();
        if let Some(item) = items.first() {
            state.open(vec![item.identifier().clone()]);
        }
        Self { state, items }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let widget = Tree::new(&self.items)
            .expect("all item identifiers are unique")
            .highlight_style(
                Style::new()
                    .fg(Color::White)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            );
        frame.render_stateful_widget(widget, area, &mut self.state);
    }

    pub fn expand(&mut self, path: &str, theme: &Theme) -> anyhow::Result<()> {
        expand_path_in_tree_items(&mut self.items, path, &std::env::current_dir()?, theme)?;
        Ok(())
    }

    pub fn refresh(&mut self, theme: &Theme) -> anyhow::Result<()> {
        let root_path = std::env::current_dir()?;
        let mut opened_paths = self.state.opened().iter().cloned().collect::<Vec<_>>();
        opened_paths.sort_by_key(|path| path.len());

        self.items = build_initial_tree_items(&root_path, theme);

        for opened_path in opened_paths {
            if let Some(target_path) = opened_path.last() {
                self.expand(target_path, theme)?;
            }
        }
        Ok(())
    }

    pub fn open_file_path(&mut self, filename: &str, theme: &Theme) {
        let root_path = match std::env::current_dir() {
            Ok(p) => p,
            Err(_) => return,
        };

        let file_path = Path::new(filename);
        let abs_file_path = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            root_path.join(file_path)
        };

        if !abs_file_path.starts_with(&root_path) {
            return;
        }

        let root_id = root_path.to_string_lossy().into_owned();
        let mut open_path = vec![root_id.clone()];
        let mut select_path = vec![root_id];

        let mut current_path = root_path.clone();
        if let Some(rel_path) = abs_file_path
            .parent()
            .and_then(|p| p.strip_prefix(&root_path).ok())
        {
            for component in rel_path.iter() {
                current_path = current_path.join(component);
                let dir_id = current_path.to_string_lossy().into_owned();

                let _ = expand_path_in_tree_items(&mut self.items, &dir_id, &root_path, theme);

                open_path.push(dir_id.clone());
                select_path.push(dir_id);
            }
        }

        let file_id = abs_file_path.to_string_lossy().into_owned();
        select_path.push(file_id);

        for i in 0..open_path.len() {
            let sub_path = open_path[0..=i].to_vec();
            self.state.open(sub_path);
        }
        self.state.select(select_path);
    }

    /// Returns the selected path if a leaf (file) was activated
    #[allow(dead_code)]
    pub fn selected_path(&self) -> Option<String> {
        let selected = self.state.selected();
        if selected.is_empty() {
            return None;
        }
        selected.last().map(|s| s.to_string())
    }

    // Navigation helpers
    pub fn click_at(&mut self, pos: Position) {
        self.state.click_at(pos);
    }
    pub fn toggle_selected(&mut self) {
        self.state.toggle_selected();
    }
    pub fn key_left(&mut self) {
        self.state.key_left();
    }
    pub fn key_right(&mut self) {
        self.state.key_right();
    }
    pub fn key_down(&mut self) {
        self.state.key_down();
    }
    pub fn key_up(&mut self) {
        self.state.key_up();
    }
    pub fn select_first(&mut self) {
        self.state.select_first();
    }
    pub fn select_last(&mut self) {
        self.state.select_last();
    }
    pub fn scroll_down(&mut self, n: usize) {
        self.state.scroll_down(n);
    }
    pub fn scroll_up(&mut self, n: usize) {
        self.state.scroll_up(n);
    }

    pub fn handle_event(&mut self, event: &Event, area: Rect, theme: &Theme) -> TreeAction {
        let mut check_selected = false;
        let opened_before = self.state.opened().clone();

        match event {
            Event::Key(key) if !matches!(key.kind, KeyEventKind::Press) => {}
            Event::Key(key) => match key.code {
                KeyCode::Char('q') => return TreeAction::Quit,
                KeyCode::Enter => {
                    self.toggle_selected();
                    check_selected = true;
                }
                KeyCode::Left => self.key_left(),
                KeyCode::Right => self.key_right(),
                KeyCode::Down => self.key_down(),
                KeyCode::Up => self.key_up(),
                KeyCode::Home => self.select_first(),
                KeyCode::End => self.select_last(),
                KeyCode::PageDown => self.scroll_down(3),
                KeyCode::PageUp => self.scroll_up(3),
                _ => {}
            },
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollDown => {
                    let offset_y = self.state.get_offset();
                    let len = self.state.flatten(&self.items).len();
                    let area_height = area.height as usize;
                    if offset_y < len.saturating_sub(area_height) {
                        self.scroll_down(1);
                    }
                }
                MouseEventKind::ScrollUp => self.scroll_up(1),
                MouseEventKind::Down(_) => {
                    self.click_at(Position::new(mouse.column, mouse.row));
                    check_selected = true;
                }
                _ => {}
            },
            _ => {}
        }

        let selected = self.state.selected().to_vec();
        let opened_changed = self.state.opened() != &opened_before;

        if check_selected && !selected.is_empty() {
            let name = selected.last().unwrap().to_string();
            let path = Path::new(&name);
            if path.is_dir() {
                if self.state.opened().contains(&selected) {
                    let _ = self.expand(&name, theme);
                }
            } else {
                return TreeAction::OpenFile(name);
            }
        } else if opened_changed && !selected.is_empty() {
            let name = selected.last().unwrap().to_string();
            let path = Path::new(&name);
            if path.is_dir() && self.state.opened().contains(&selected) {
                let _ = self.expand(&name, theme);
            }
        }

        TreeAction::None
    }
}

pub fn should_refresh_tree(event: &notify::Event) -> bool {
    matches!(
        event.kind,
        notify::EventKind::Create(_)
            | notify::EventKind::Remove(_)
            | notify::EventKind::Modify(ModifyKind::Name(_))
    )
}

pub fn build_tree_items(path: &Path, theme: &Theme) -> Vec<TreeItem<'static, String>> {
    let mut folders = Vec::new();
    let mut files = Vec::new();

    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if is_ignored_path(&path) {
                continue;
            }

            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let abs_path = path.to_string_lossy().into_owned();

            if path.is_dir() {
                let color = find_color(theme, "type").unwrap_or_default();
                let name = Span::styled(name, Style::default().fg(color));
                if let Ok(item) = TreeItem::new(abs_path, name, vec![]) {
                    folders.push(item);
                }
            } else {
                let color = find_color(theme, "variable").unwrap_or_default();
                let name = Span::styled(name, Style::default().fg(color));
                files.push(TreeItem::new_leaf(abs_path, name));
            }
        }
    }

    let mut items = Vec::with_capacity(folders.len() + files.len());
    items.extend(folders);
    items.extend(files);
    items
}

pub fn build_initial_tree_items(root_path: &Path, theme: &Theme) -> Vec<TreeItem<'static, String>> {
    let child_items = build_tree_items(root_path, theme);

    // Create root tree item containing all children
    let root_name = root_path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("."))
        .to_string_lossy()
        .into_owned();

    let root_identifier = root_path.to_string_lossy().into_owned();

    let items = match TreeItem::new(root_identifier, root_name, child_items.clone()) {
        Ok(root_item) => vec![root_item],
        Err(_) => child_items,
    };
    items
}

pub fn expand_path_in_tree_items(
    items: &mut [TreeItem<'static, String>],
    target_path: &str,
    root_path: &Path,
    theme: &Theme,
) -> std::io::Result<bool> {
    for i in 0..items.len() {
        let item = &mut items[i];

        let found = item.identifier() == target_path;
        if found {
            // target_path is now an absolute path, use it directly
            let full_path = Path::new(target_path);
            if full_path.is_dir() {
                let children = build_tree_items(full_path, theme);
                for child in children {
                    let _ = item.add_child(child);
                }
                return Ok(true);
            }
        }

        // recursively find and expand children
        for child_idx in 0..item.children().len() {
            if let Some(child) = item.child_mut(child_idx) {
                let found = expand_path_in_tree_items(
                    std::slice::from_mut(child),
                    target_path,
                    root_path,
                    theme,
                )?;
                if found {
                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
}