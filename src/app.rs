use anyhow::Result;
use crossterm::event::EventStream;
use crossterm::event::{Event, KeyCode, KeyModifiers, MouseEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::{DefaultTerminal, Frame};
use std::collections::HashSet;
use std::path::PathBuf;
use tokio_stream::StreamExt;

use crate::editor::{EditorAction, EditorPanel, Fallback};
use crate::llm::LlmClient;
use crate::search::{SearchAction, SearchMode, SearchPanel};
use crate::tree::{TreeAction, TreePanel, should_refresh_tree};
use crate::utils::{abs_file, is_focused};
use crate::watcher::FsWatcher;

pub type Theme = Vec<(&'static str, &'static str)>;

#[derive(PartialEq)]
pub enum LeftPanelMode {
    Tree,
    Search,
}

pub enum Message {
    Quit,
    ToggleLeftPanel,
    ActivateSearch(SearchMode),
    SearchAction(SearchAction),
    SearchUpdate(crate::search::SearchUpdate),
    OpenFile(String),
    SaveCurrentFile,
    FileChangedExternally(notify::Event),
    AutocompleteResult(Result<Vec<crate::diff::Edit>>),
    None,
}

pub struct LeftPanel {
    visible: bool,
    focused: bool,
    mode: LeftPanelMode,
    area: Rect,
    split_ratio: usize,
    is_resizing: bool,
    pub tree: TreePanel,
    pub search: SearchPanel,
}

pub struct App {
    quit: bool,
    theme: Theme,
    left_panel: LeftPanel,
    editor_panel: EditorPanel,
    watcher: FsWatcher,
}

impl App {
    pub fn new(
        language: &str,
        content: &str,
        filename: &str,
        llm_client: Option<LlmClient>,
    ) -> Result<Self> {
        let root_path = std::env::current_dir().unwrap();
        let theme = ratatui_code_editor::theme::vesper();
        let left_panel_visible = filename.is_empty();

        let tree = TreePanel::new(&root_path, &theme);
        let search = SearchPanel::new();
        let center = EditorPanel::new(language, content, filename, llm_client)?;

        let left = LeftPanel {
            visible: left_panel_visible,
            focused: left_panel_visible,
            mode: LeftPanelMode::Tree,
            area: Rect::default(),
            split_ratio: 20,
            is_resizing: false,
            tree,
            search,
        };

        let mut app = Self {
            quit: false,
            theme: theme.clone(),
            left_panel: left,
            editor_panel: center,
            watcher: FsWatcher::new(),
        };

        if !filename.is_empty() {
            app.left_panel.tree.open_file_path(filename, &app.theme);
        }
        app.sync_watch_paths()?;

        Ok(app)
    }

    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        let mut events = EventStream::new();
        terminal.draw(|frame| self.render(frame))?;

        while !self.quit {
            let msg = tokio::select! {
                maybe_event = events.next() => {
                    match maybe_event {
                        Some(Ok(event)) => self.handle_event(&event),
                        _ => Message::None,
                    }
                }
                result = self.editor_panel.recv_autocomplete() => {
                    match result {
                        Some(r) => Message::AutocompleteResult(r),
                        _ => Message::None,
                    }
                }
                event = self.watcher.watch_rx.recv() => {
                    match event {
                        Some(Ok(e)) => Message::FileChangedExternally(e),
                        _ => Message::None,
                    }
                }
                update = self.left_panel.search.recv() => {
                    match update {
                        Some(u) => Message::SearchUpdate(u),
                        _ => Message::None,
                    }
                }
            };

            self.update(msg).await?;
            terminal.draw(|frame| self.render(frame))?;
        }

        Ok(())
    }

    fn render(&mut self, frame: &mut Frame) {
        let left_ratio = if self.left_panel.visible {
            self.left_panel.split_ratio as u16
        } else {
            0
        };
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(left_ratio),
                Constraint::Percentage(100 - left_ratio),
            ])
            .split(frame.area());

        self.left_panel.area = chunks[0];
        self.editor_panel.area = chunks[1];

        if self.left_panel.visible {
            match self.left_panel.mode {
                LeftPanelMode::Search => self.left_panel.search.render(frame, self.left_panel.area),
                LeftPanelMode::Tree => self.left_panel.tree.render(frame, self.left_panel.area),
            }
        }

        self.editor_panel.render(frame);
    }

    fn handle_event(&mut self, event: &Event) -> Message {
        match event {
            Event::Key(key) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('t') {
                    return Message::ToggleLeftPanel;
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(' ') {
                    self.editor_panel.spawn_autocomplete();
                    return Message::None;
                }
            }
            Event::Mouse(mouse) => {
                if !self.left_panel.visible {
                    self.left_panel.focused = false;
                    return self.handle_editor_event(event);
                }

                self.left_panel.focused = is_focused(mouse, self.left_panel.area);
                let splitter_x = self.left_panel.area.x + self.left_panel.area.width;

                match mouse.kind {
                    MouseEventKind::Down(_) => {
                        if mouse.column == splitter_x || mouse.column == splitter_x + 1 {
                            self.left_panel.is_resizing = true;
                        } else {
                            self.left_panel.focused = is_focused(mouse, self.left_panel.area);
                        }
                    }
                    MouseEventKind::Drag(_) => {
                        if self.left_panel.is_resizing {
                            let total = self.left_panel.area.width + self.editor_panel.area.width + 2;
                            let ratio = (mouse.column as f32 / total as f32 * 100.0) as u16;
                            self.left_panel.split_ratio = ratio.clamp(0, 100) as usize;
                        }
                    }
                    MouseEventKind::Up(_) => self.left_panel.is_resizing = false,
                    _ => {}
                }
            }
            _ => {}
        }

        if self.left_panel.is_resizing {
            return Message::None;
        }

        if self.left_panel.focused {
            match self.left_panel.mode {
                LeftPanelMode::Search => {
                    let action = self.left_panel.search.handle_event(event, self.left_panel.area);
                    Message::SearchAction(action)
                }
                LeftPanelMode::Tree => {
                    let action = self
                        .left_panel
                        .tree
                        .handle_event(event, self.left_panel.area, &self.theme);
                    match action {
                        TreeAction::OpenFile(path) => Message::OpenFile(path),
                        TreeAction::Quit => Message::Quit,
                        TreeAction::None => Message::None,
                    }
                }
            }
        } else {
            self.handle_editor_event(event)
        }
    }

    fn handle_editor_event(&mut self, event: &Event) -> Message {
        match self.editor_panel.handle_event(event) {
            EditorAction::Quit => Message::Quit,
            EditorAction::ActivateSearch(mode) => Message::ActivateSearch(mode),
            EditorAction::Save => Message::SaveCurrentFile,
            EditorAction::None => Message::None,
        }
    }

    async fn update(&mut self, msg: Message) -> Result<()> {
        match msg {
            Message::Quit => self.quit = true,
            Message::ToggleLeftPanel => self.toggle_left_panel(),
            Message::ActivateSearch(mode) => self.activate_search(mode)?,
            Message::SearchAction(action) => self.process_search_action(action).await?,
            Message::SearchUpdate(update) => self.left_panel.search.apply_update(update),
            Message::OpenFile(path) => self.open_file(&path).await?,
            Message::SaveCurrentFile => self.editor_panel.save().await?,
            Message::FileChangedExternally(e) => self.handle_file_change(e).await?,
            Message::AutocompleteResult(r) => self.editor_panel.handle_autocomplete(r).await?,
            Message::None => {}
        }
        Ok(())
    }

    fn toggle_left_panel(&mut self) {
        if !self.left_panel.visible {
            self.left_panel.visible = true;
            self.left_panel.mode = LeftPanelMode::Tree;
            self.left_panel.focused = true;
        } else {
            self.left_panel.visible = false;
            self.left_panel.focused = false;
        }
    }

    fn activate_search(&mut self, mode: SearchMode) -> Result<()> {
        self.editor_panel.fallback = Some(Fallback {
            filename: self.editor_panel.filename.clone(),
            cursor: self.editor_panel.editor.get_cursor(),
            selection: self.editor_panel.editor.get_selection(),
            offsets: (
                self.editor_panel.editor.get_offset_y(),
                self.editor_panel.editor.get_offset_x(),
            ),
            left_panel_visible: self.left_panel.visible,
        });
        self.left_panel.search.activate(mode);
        self.left_panel.mode = LeftPanelMode::Search;
        self.left_panel.visible = true;
        self.left_panel.focused = true;

        if let Some(q) = self.editor_panel.editor.get_selection_text() {
            self.left_panel.search.query = q;
            if self.left_panel.search.mode == SearchMode::GlobalSearch {
                self.left_panel
                    .search
                    .start_global_search(std::env::current_dir()?);
            } else {
                let content = self.editor_panel.editor.get_content();
                self.left_panel.search.search(&content);
            }
        }
        Ok(())
    }

    async fn process_search_action(&mut self, action: SearchAction) -> Result<()> {
        match action {
            SearchAction::UpdateSearch => {
                if self.left_panel.search.mode == SearchMode::GlobalSearch {
                    self.left_panel
                        .search
                        .start_global_search(std::env::current_dir()?);
                } else {
                    let content = self.editor_panel.editor.get_content();
                    self.left_panel.search.search(&content);
                }
            }
            SearchAction::Clear => {
                self.left_panel.search.results.clear();
                self.left_panel.search.selected = None;
                self.left_panel.search.scroll_offset = 0;
            }
            SearchAction::JumpTo(result) => {
                if let Some(file_path) = &result.file_path {
                    self.open_file(file_path).await?;
                }
                self.editor_panel.editor.set_cursor(result.match_start);
                self.editor_panel.editor.focus(&self.editor_panel.area);
                let marks = vec![(result.match_start, result.match_end, "#585858")];
                self.editor_panel.editor.set_marks(marks);
                self.left_panel.focused = true;
            }
            SearchAction::JumpToAndExit(result) => {
                if let Some(file_path) = &result.file_path {
                    self.open_file(file_path).await?;
                }
                let left_visible = self
                    .editor_panel
                    .fallback
                    .take()
                    .map(|f| f.left_panel_visible)
                    .unwrap_or(self.left_panel.visible);
                self.left_panel.mode = LeftPanelMode::Tree;
                self.left_panel.visible = left_visible;
                self.editor_panel.editor.set_cursor(result.match_start);
                self.editor_panel.editor.focus(&self.editor_panel.area);
                self.left_panel.focused = false;
                self.editor_panel.editor.remove_marks();
            }
            SearchAction::Close => {
                self.left_panel.search.cancel();
                if let Some(fallback) = self.editor_panel.fallback.take() {
                    let _ = self.editor_panel.open_file(&fallback.filename).await;
                    self.editor_panel.editor.set_cursor(fallback.cursor);
                    if let Some(selection) = fallback.selection {
                        self.editor_panel.editor.set_selection(Some(selection));
                    }
                    self.editor_panel.editor.set_offset_y(fallback.offsets.0);
                    self.editor_panel.editor.set_offset_x(fallback.offsets.1);
                    self.editor_panel.editor.focus(&self.editor_panel.area);
                    self.left_panel.visible = fallback.left_panel_visible;
                }
                self.left_panel.mode = LeftPanelMode::Tree;
                self.left_panel.focused = false;
            }
            SearchAction::None => {}
        }
        Ok(())
    }

    async fn open_file(&mut self, path: &str) -> Result<()> {
        self.editor_panel.open_file(path).await?;
        self.left_panel.tree.open_file_path(path, &self.theme);
        self.sync_watch_paths()?;
        self.left_panel.focused = false;
        Ok(())
    }

    async fn handle_file_change(&mut self, event: notify::Event) -> Result<()> {
        if should_refresh_tree(&event) {
            self.left_panel.tree.refresh(&self.theme)?;
            self.sync_watch_paths()?;
        }
        self.editor_panel.handle_file_change(&event).await?;
        Ok(())
    }

    fn sync_watch_paths(&mut self) -> Result<()> {
        let mut watch_paths = self
            .left_panel
            .tree
            .state
            .opened()
            .iter()
            .filter_map(|p| p.last())
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .collect::<HashSet<_>>();

        if !self.editor_panel.filename.is_empty() {
            watch_paths.insert(PathBuf::from(abs_file(&self.editor_panel.filename)));
        }
        self.watcher.sync(watch_paths)?;
        Ok(())
    }
}
