use std::path::PathBuf;
use crossterm::{
    event::{
        Event, KeyCode, KeyModifiers, KeyEvent, KeyEventKind,
        MouseEventKind, MouseEvent
    },
};
use anyhow::Result;
use crossterm::event::{EventStream};
use ratatui::layout::{Layout, Constraint, Direction, Rect, Position};
use ratatui::{DefaultTerminal, Frame};
use ratatui::style::{Color, Modifier, Style};
use ratatui_code_editor::editor::Editor;
use ratatui_code_editor::selection::Selection;
use ratatui_code_editor::theme::vesper;
use ratatui_code_editor::utils::get_lang;
use ratatui_code_editor::code::{EditKind, EditBatch, EditState};
use ratatui::widgets::{Paragraph, Wrap};
use std::fs;
use std::io::Write;
use tokio_stream::StreamExt;
use tokio::task::JoinHandle;
use tokio::sync::mpsc;
use std::sync::Arc;
use tokio::sync::Mutex;
use tui_tree_widget::{Tree, TreeItem, TreeState};
use std::path::{Path};
use std::collections::HashMap;

use crate::diff::compute_changed_ranges_normalized;
use crate::coder::Coder;
use crate::llm::LlmClient;
use crate::diff::ChangedRangeKind;
use crate::diff::EditKind::*;
use crate::diff::Edit;
use crate::utils::{abs_file};
use crate::watcher::FsWatcher;
use crate::search::{SearchPanel, SearchAction, SearchMode, SearchUpdate};
use crate::tree::{build_initial_tree_items, expand_path_in_tree_items};
use notify::event::ModifyKind;

const COLOR_INSERT: &str = "#02a365";
const COLOR_DELETE: &str = "#f6c99f";

#[derive(PartialEq)]
pub enum LeftPanelMode {
    Tree,
    Search,
}

#[derive(Clone, Debug)]
pub struct Fallback {
    filename: String,
    cursor: usize,
    offsets: (usize, usize),
    selection: Option<Selection>,
    left_panel_visible: bool,
}

pub type Theme = Vec<(&'static str, &'static str)>;

pub struct App {
    editor: Editor,
    editor_area: Rect,
    filename: String,
    opened_editors: HashMap<String, Editor>,
    fallback: Option<Fallback>,
    theme: Theme,
    quit: bool,
    split_ratio: usize,
    is_resizing: bool,
    left_panel_visible: bool,
    left_panel_focused: bool,
    tree_area: Rect,
    tree_state: TreeState<String>,
    tree_items: Vec<TreeItem<'static, String>>,
    coder: Arc<Mutex<Coder>>,
    autocomplete_handle: Option<JoinHandle<()>>,
    autocomplete_tx: mpsc::Sender<Result<Vec<Edit>>>,
    autocomplete_rx: mpsc::Receiver<Result<Vec<Edit>>>,
    watcher: FsWatcher,
    self_update: bool,
    search_panel: SearchPanel,
    left_panel_mode: LeftPanelMode,
    search_rx: mpsc::UnboundedReceiver<SearchUpdate>,
    search_tx: mpsc::UnboundedSender<SearchUpdate>,
    search_handle: Option<JoinHandle<()>>,
}

impl App {
    pub fn new(
        language: &str, content: &str,
        filename: &str, llm_client: LlmClient
    ) -> Result<Self> {
        let root_path = std::env::current_dir().unwrap();
        let theme = vesper();
        let items = build_initial_tree_items(&root_path, &theme);
        let editor = Editor::new(language, content, theme.clone())?;
        let mut coder = Coder::new(llm_client);
        coder.update(&PathBuf::from(filename), &content);
        let (tx, rx) = mpsc::channel(1);
        let (search_tx, search_rx) = mpsc::unbounded_channel();
        let left_panel_visible = filename.is_empty();
        let tree_focused = left_panel_visible;
        let watcher = FsWatcher::new();
        let search_tx_clone = search_tx.clone();

        let mut tree_state = TreeState::default();

        if let Some(item) = items.first() {
            tree_state.open(vec![item.identifier().clone()]);
        }

        Ok(Self {
            quit: false,
            editor,
            opened_editors: HashMap::new(),
            editor_area: Rect::default(),
            filename: filename.to_string(),
            fallback: None,
            theme: theme,
            coder: Arc::new(Mutex::new(coder)),
            autocomplete_handle: None,
            autocomplete_tx: tx,
            autocomplete_rx: rx,
            split_ratio: 20,
            is_resizing: false,
            left_panel_visible,
            left_panel_focused: tree_focused,
            tree_area: Rect::default(),
            tree_state: tree_state,
            tree_items: items,
            watcher,
            self_update: false,
            search_panel: SearchPanel::new(),
            left_panel_mode: LeftPanelMode::Tree,
            search_rx,
            search_tx: search_tx_clone,
            search_handle: None,
        })
    }

    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        let mut events = EventStream::new();

        terminal.draw(|frame| self.render(frame))?;

        while !self.quit {
            tokio::select! {
                maybe_event = events.next() => {
                    if let Some(Ok(event)) = maybe_event {
                        self.handle_event(&event).await?;
                        terminal.draw(|frame| self.render(frame))?;
                    }
                }
                result = self.autocomplete_rx.recv() => {
                    if let Some(result) = result {
                        self.handle_autocomplete(result).await?;
                        terminal.draw(|frame| self.render(frame))?;
                    }
                }
                event = self.watcher.watch_rx.recv() => {
                    if let Some(Ok(result)) = event {
                        self.handle_watch_event(result).await?;
                        terminal.draw(|frame| self.render(frame))?;
                    }
                }
                update = self.search_rx.recv() => {
                    if let Some(update) = update {
                        self.handle_search_update(update).await?;
                        terminal.draw(|frame| self.render(frame))?;
                    }
                }
            }
        }

        Ok(())
    }

    fn render(&mut self, frame: &mut Frame) {
        let left_panel_ratio = if self.left_panel_visible {
            self.split_ratio as u16
        } else {
            0
        };
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(left_panel_ratio),
                Constraint::Percentage(100 - left_panel_ratio),
            ])
            .split(frame.area());

        self.tree_area = chunks[0];
        self.editor_area = chunks[1];

        // Render left panel based on mode
        if self.left_panel_visible {
            if self.left_panel_mode == LeftPanelMode::Search {
                // Render search panel
                self.search_panel.render(frame, self.tree_area);
            } else {
                // Render tree panel
                let widget = Tree::new(&self.tree_items)
                    .expect("all item identifiers are unique")
                    .highlight_style(
                        Style::new()
                            .fg(Color::White)
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD),
                    );
                frame.render_stateful_widget(widget, self.tree_area, &mut self.tree_state);
            }
        }

        if self.filename.is_empty() {
            let welcome = Paragraph::new(" Welcome to redai!")
                .style(Style::default().fg(Color::Reset))
                .wrap(Wrap { trim: false });
            frame.render_widget(welcome, self.editor_area);
        } else {
            frame.render_widget(&self.editor, self.editor_area);

            let cursor = self.editor.get_visible_cursor(&self.editor_area);
            if let Some((x,y)) = cursor {
                frame.set_cursor_position(Position::new(x, y));
            }
        }
    }

    async fn handle_event(&mut self, event: &Event) -> Result<()> {
        match event {
            Event::Key(key) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('t') {
                    if !self.left_panel_visible {
                        self.left_panel_visible = true;
                        self.left_panel_mode = LeftPanelMode::Tree;
                        self.left_panel_focused = true;
                    } else {
                        self.left_panel_visible = false;
                        self.left_panel_focused = false;
                    }
                    return Ok(());
                }
                if is_autocomplete_pressed(*key) {
                    self.spawn_autocomplete_task();
                    return Ok(());
                }
            }
            Event::Mouse(mouse) => {
                if !self.left_panel_visible {
                    self.left_panel_focused = false;
                    return self.handle_editor_event(event).await;
                }

                self.left_panel_focused = is_focused(mouse, self.tree_area);

                let splitter_x = self.tree_area.x + self.tree_area.width;

                match mouse.kind {
                    MouseEventKind::Down(_) => {
                        if mouse.column == splitter_x || mouse.column == splitter_x+1 {
                            self.is_resizing = true;
                        } else {
                            self.left_panel_focused = is_focused(mouse, self.tree_area);
                        }
                    }
                    MouseEventKind::Drag(_) => {
                        if self.is_resizing {
                            let total_width = self.tree_area.width + self.editor_area.width + 2;
                            let new_ratio = (mouse.column as f32 / total_width as f32 * 100.0) as u16;
                            self.split_ratio = new_ratio.clamp(0, 100) as usize;
                        }
                    }
                    MouseEventKind::Up(_) => {
                        self.is_resizing = false;
                    }
                    _ => {}
                }
            }
            _ => {},
        }

        if self.is_resizing { return Ok(());}

        if self.left_panel_focused {
            if self.left_panel_mode == LeftPanelMode::Search {
                self.handle_search_event(event).await?;
            } else {
                self.handle_tree_event(event).await?;
            }
        } else {
            self.handle_editor_event(event).await?;
        }

        Ok(())
    }

    async fn handle_editor_event(&mut self, event: &Event) -> Result<()> {
        match event {
            Event::Paste(paste) => {
                self.editor.apply(ratatui_code_editor::actions::InsertText{
                    text: paste.to_string()
                });
            }
            Event::Key(key) => {
                // Handle search activation
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('f') {
                    self.fallback = Some(Fallback {
                        filename: self.filename.clone(),
                        cursor: self.editor.get_cursor(),
                        selection: self.editor.get_selection(),
                        offsets: (self.editor.get_offset_y(), self.editor.get_offset_x()),
                        left_panel_visible: self.left_panel_visible,
                    });
                    self.search_panel.activate(SearchMode::Search);
                    self.left_panel_mode = LeftPanelMode::Search;
                    self.left_panel_visible = true;
                    self.left_panel_focused = true;

                    // if there is a selection, use it as the search query
                    if let Some(q) = self.editor.get_selection_text() {
                        self.search_panel.query = q;
                        self.handle_search_event(event).await?;
                    }

                    return Ok(());
                }

                // Handle global search activation
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('g') {
                    self.fallback = Some(Fallback {
                        filename: self.filename.clone(),
                        cursor: self.editor.get_cursor(),
                        selection: self.editor.get_selection(),
                        offsets: (self.editor.get_offset_y(), self.editor.get_offset_x()),
                        left_panel_visible: self.left_panel_visible,
                    });
                    self.search_panel.activate(SearchMode::GlobalSearch);
                    self.left_panel_mode = LeftPanelMode::Search;
                    self.left_panel_visible = true;
                    self.left_panel_focused = true;

                    // if there is a selection, use it as the search query
                    if let Some(q) = self.editor.get_selection_text() {
                        self.search_panel.query = q;
                        self.handle_search_event(event).await?;
                    }
                    return Ok(());
                }

                // cancel autocomplete if is active, else nothing
                self.autocomplete_handle.take().map(|h| h.abort());

                let has_marks = self.editor.has_marks();

                if key.code == KeyCode::Esc && has_marks {
                    self.editor.remove_marks();
                    self.editor.apply(ratatui_code_editor::actions::Undo { });
                } else if is_quit_pressed(*key) {
                    self.quit = true;
                } else if is_autocomplete_pressed(*key) {
                    self.spawn_autocomplete_task();
                } else if is_save_pressed(*key) {
                    let content = self.editor.get_content();
                    save_to_file(&content, &self.filename)?;
                    let mut coder = self.coder.lock().await;
                    let p = PathBuf::from(&self.filename);
                    coder.update(&p, &content);
                    self.self_update = true;
                } else {
                    let accepted = key.code == KeyCode::Tab ||
                        key.code == KeyCode::Enter;
                    if has_marks {
                        if accepted {
                            self.editor.remove_marks();
                        } else {
                            self.editor.remove_marks();
                            self.editor.apply(ratatui_code_editor::actions::Undo { });
                            self.editor.input(*key, &self.editor_area)?;
                        }
                    } else {
                        self.editor.input(*key, &self.editor_area)?;
                    }
                }
            }
            Event::Mouse(mouse) => {
                self.editor.mouse(*mouse, &self.editor_area)?;
            }
            _ => {},
        };

        Ok(())
    }

    async fn handle_tree_event(&mut self, event: &Event) -> Result<()> {
        let mut check_selected = false;
        match event {
            Event::Key(key) if !matches!(key.kind, KeyEventKind::Press) => { },
            Event::Key(key) => match key.code {
                KeyCode::Char('q') => { self.quit = true; },
                KeyCode::Enter => {
                    self.tree_state.toggle_selected();
                    check_selected = true;
                },
                // KeyCode::Esc => { self.quit = true; },
                KeyCode::Left => { self.tree_state.key_left(); },
                KeyCode::Right => { self.tree_state.key_right(); },
                KeyCode::Down => { self.tree_state.key_down(); },
                KeyCode::Up => { self.tree_state.key_up(); },
                KeyCode::Home => { self.tree_state.select_first(); },
                KeyCode::End => { self.tree_state.select_last(); },
                KeyCode::PageDown => { self.tree_state.scroll_down(3); },
                KeyCode::PageUp => { self.tree_state.scroll_up(3); },
                _ => { },
            },
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollDown => {
                    let offset_y = self.tree_state.get_offset();
                    let len = self.tree_state.flatten(&self.tree_items).len();
                    let area_height = self.tree_area.height as usize;
                    if offset_y < len.saturating_sub(area_height) {
                        self.tree_state.scroll_down(1);
                    }
                },
                MouseEventKind::ScrollUp => { self.tree_state.scroll_up(1); },
                MouseEventKind::Down(_button) => {
                    self.tree_state.click_at(Position::new(mouse.column, mouse.row));
                    check_selected = true;
                },
                _ => { },
            },
            Event::Resize(_, _) => { },
            _ => { },
        };

        let selected = self.tree_state.selected();

        if check_selected && !selected.is_empty() {
            let name = selected.last().unwrap().to_string();

            let path = Path::new(&name);
            if path.is_dir() {
                expand_path_in_tree_items(
                    &mut self.tree_items, &name, &std::env::current_dir()?, &self.theme
                )?;
                return Ok(())
            }
            else {
                self.open_file(&name).await?;
            }
        }

        Ok(())
    }

    async fn handle_search_event(&mut self, event: &Event) -> Result<()> {
        match event {
            Event::Paste(paste) => {
                self.search_panel.query.push_str(&paste.to_string());
                let action = if self.search_panel.mode == SearchMode::GlobalSearch {
                    SearchAction::None
                } else {
                    SearchAction::UpdateSearch
                };
                self.process_search_action(action).await?;
            }
            Event::Key(key) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    let mode = match key.code {
                        KeyCode::Char('f') => Some(SearchMode::Search),
                        KeyCode::Char('g') => Some(SearchMode::GlobalSearch),
                        _ => None,
                    };

                    if let Some(new_mode) = mode {
                        self.search_panel.mode = new_mode;
                        if !self.search_panel.query.is_empty() {
                            self.process_search_action(SearchAction::UpdateSearch).await?;
                        }
                        return Ok(());
                    }
                }

                let action = self.search_panel.handle_input(*key, self.tree_area);
                self.process_search_action(action).await?;
            }
            Event::Mouse(mouse) => {
                match mouse.kind {
                    crossterm::event::MouseEventKind::Down(_) => {
                        let action = self.search_panel.handle_mouse_click(mouse, self.tree_area);
                        self.process_search_action(action).await?;
                    }
                    crossterm::event::MouseEventKind::ScrollDown => {
                        self.search_panel.scroll_down(self.tree_area);
                    }
                    crossterm::event::MouseEventKind::ScrollUp => {
                        self.search_panel.scroll_up();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_search_update(&mut self, update: SearchUpdate) -> Result<()> {
        match update {
            SearchUpdate::Progress { processed, total } => {
                self.search_panel.search_progress = Some((processed, total));
            }
            SearchUpdate::Results(new_results) => {
                self.search_panel.results.extend(new_results);
                if !self.search_panel.results.is_empty() && self.search_panel.selected.is_none() {
                    self.search_panel.selected = Some(0);
                }
            }
            SearchUpdate::Finished { files_processed, duration } => {
                if self.search_handle.is_some() {
                    self.search_panel.search_in_progress = false;
                    self.search_panel.search_time = Some(duration);
                    self.search_panel.files_processed = Some(files_processed);
                    self.search_panel.search_progress = None;
                }
                self.search_handle = None;
            }
        }
        Ok(())
    }

    async fn process_search_action(&mut self, action: SearchAction) -> Result<()> {
        match action {
            SearchAction::UpdateSearch => {
                if self.search_panel.mode == SearchMode::GlobalSearch {
                    // cancel previous search if it's in progress
                    if let Some(handle) = self.search_handle.take() {
                        handle.abort();
                    }

                    self.search_panel.results.clear();
                    self.search_panel.selected = None;
                    self.search_panel.scroll_offset = 0;
                    self.search_panel.search_in_progress = true;
                    self.search_panel.search_progress = None;

                    // spawn global search in a separate task
                    let handle = SearchPanel::spawn_global_search(
                        std::env::current_dir()?,
                        self.search_panel.query.clone(),
                        self.search_panel.case_sensitive,
                        self.search_panel.regex_mode,
                        self.search_tx.clone(),
                    );
                    self.search_handle = Some(handle);
                } else {
                    // local search in current file
                    let content = self.editor.get_content();
                    self.search_panel.search(&content);
                }
            }
            SearchAction::Clear => {
                self.search_panel.results.clear();
                self.search_panel.selected = None;
                self.search_panel.scroll_offset = 0;
            }
            SearchAction::JumpTo(result) => {
                if let Some(file_path) = &result.file_path {
                    // global search - open file and jump to position
                    self.open_file(file_path).await?;
                    let cursor_pos = result.match_start;
                    self.editor.set_cursor(cursor_pos);
                    self.editor.focus(&self.editor_area);

                    let marks = vec![(result.match_start,result.match_end, "#585858")];
                    self.editor.set_marks(marks);
                } else {
                    // local search in current file
                    let cursor_pos = result.match_start;
                    self.editor.set_cursor(cursor_pos);
                    self.editor.focus(&self.editor_area);
                    let marks = vec![(result.match_start,result.match_end,"#585858")];
                    self.editor.set_marks(marks);
                }
                self.left_panel_focused = true;
            }
            SearchAction::JumpToAndExit(result) => {
                if let Some(file_path) = &result.file_path {
                    self.open_file(file_path).await?;
                }

                let left_panel_visible = self.fallback
                    .take()
                    .map(|fallback| fallback.left_panel_visible)
                    .unwrap_or(self.left_panel_visible);

                self.left_panel_mode = LeftPanelMode::Tree;
                self.left_panel_visible = left_panel_visible;
                let cursor_pos = result.match_start;
                self.editor.set_cursor(cursor_pos);
                self.editor.focus(&self.editor_area);
                self.left_panel_focused = false;
                self.editor.remove_marks();
            }
            SearchAction::Close => {
                // Cancel search if it's in progress
                if let Some(handle) = self.search_handle.take() {
                    handle.abort();
                    self.search_panel.search_in_progress = false;
                    self.search_panel.search_progress = None;
                }

                if let Some(fallback) = self.fallback.take() {
                    let _ = self.open_file(&fallback.filename).await;
                    self.editor.set_cursor(fallback.cursor);
                    if let Some(selection) = fallback.selection {
                        self.editor.set_selection(Some(selection));
                    }
                    self.editor.set_offset_y(fallback.offsets.0);
                    self.editor.set_offset_x(fallback.offsets.1);
                    self.editor.focus(&self.editor_area);
                    self.left_panel_visible = fallback.left_panel_visible;
                }
                self.left_panel_mode = LeftPanelMode::Tree;
                self.left_panel_focused = false;
            }
            SearchAction::None => {}
        }
        Ok(())
    }

    async fn handle_watch_event(&mut self, event: notify::Event) -> Result<()>{
        match event.kind {
            notify::EventKind::Modify(ModifyKind::Data(_)) => {
                if self.self_update {
                    self.self_update = false;
                    return Ok(());
                }
                let self_abs = abs_file(&self.filename);
                let self_path = Path::new(&self_abs);
                let is_need_self_update = event.paths.iter().any(|p| p == &self_path);
                if is_need_self_update {
                    let content = fs::read_to_string(&self_path)?;
                    self.self_update = false;
                    self.editor.set_content(&content);
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub async fn open_file(&mut self, filename: &str) -> Result<()> {
        if self.filename == filename || Path::new(filename).is_dir() {
            return Ok(())
        }

        // get or create editor for filename
        let mut new_editor = match self.opened_editors.remove(filename) {
            Some(ed) => ed,
            None => {
                let theme = vesper();
                let mut lang = get_lang(filename);
                if lang == "unknown" {
                    lang = "shell".to_string();
                }
                let content = fs::read_to_string(filename)?;
                Editor::new(&lang, &content, theme)?
            }
        };

        // swap with current, save old if existed
        if !self.filename.is_empty() {
            std::mem::swap(&mut self.editor, &mut new_editor);
            self.opened_editors.insert(self.filename.clone(), new_editor);
        } else {
            self.editor = new_editor;
        }

        // update coder and state
        let content = self.editor.get_content();
        {
            let mut coder = self.coder.lock().await;
            coder.update(&PathBuf::from(filename), &content);
        }

        self.filename = filename.to_string();
        self.watcher.add(Path::new(filename))?;

        self.open_file_in_tree(filename);

        self.left_panel_focused = false;
        Ok(())
    }

    pub fn open_file_in_tree(&mut self, filename: &str) {
        let root_path = match std::env::current_dir() {
            Ok(p) => p,
            Err(_) => return,
        };

        // Get absolute path of the file
        let file_path = Path::new(filename);
        let abs_file_path = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            root_path.join(file_path)
        };

        // Verify file is within root
        if !abs_file_path.starts_with(&root_path) {
            return; // File is outside root, skip
        }

        // Build path from root to file using absolute paths
        let root_id = root_path.to_string_lossy().into_owned();
        let mut open_path = vec![root_id.clone()];
        let mut select_path = vec![root_id];

        // Build path by traversing from root to file's parent directory
        let mut current_path = root_path.clone();
        if let Some(rel_path) = abs_file_path.parent().and_then(|p| p.strip_prefix(&root_path).ok()) {
            for component in rel_path.iter() {
                current_path = current_path.join(component);
                let dir_id = current_path.to_string_lossy().into_owned();

                // Expand this directory in tree
                let _ = expand_path_in_tree_items(
                    &mut self.tree_items,
                    &dir_id,
                    &root_path,
                    &self.theme,
                );

                open_path.push(dir_id.clone());
                select_path.push(dir_id);
            }
        }

        // Add the file itself
        let file_id = abs_file_path.to_string_lossy().into_owned();
        select_path.push(file_id);

        // Open all parent directories and select the file
        // Open each incremental path from the root to the file to expand all parent directories.
        for i in 0..open_path.len() {
            let sub_path = open_path[0..=i].to_vec();
            self.tree_state.open(sub_path);
        }
        // self.tree_state.open(open_path);
        self.tree_state.select(select_path);
    }

    fn spawn_autocomplete_task(&mut self) {
        let tx = self.autocomplete_tx.clone();
        let content = self.editor.get_content();
        let filename = self.filename.clone();
        let cursor = self.editor.get_cursor();
        let coder = self.coder.clone();

        let handle = tokio::spawn(async move {
            let coder = coder.lock().await;
            let result = coder.autocomplete(&content, &filename, cursor).await;
            let _ = tx.send(result).await;
        });
        self.autocomplete_handle = Some(handle);
    }

    async fn handle_autocomplete(
        &mut self, result: Result<Vec<Edit>>
    ) -> Result<()> {
        match result {
            Ok(edits) => {
                self.apply_edits(edits).await?
            }
            Err(err) => {
                eprintln!("autocomplete error: {err:#}");
            }
        }
        Ok(())
    }

    async fn apply_edits(
        &mut self, edits: Vec<crate::diff::Edit>
    ) -> Result<()> {

        if edits.is_empty() { return Ok(()) }

        // calculate changed ranges to visualize
        let changed_ranges = compute_changed_ranges_normalized(&edits);

        // map edits to ratatui-code-editor edit type
        let editor_edits = edits.iter().map(|e| {
            let kind = match &e.kind {
                Insert => EditKind::Insert { offset: e.start, text: e.text.clone() },
                Delete => EditKind::Remove { offset: e.start, text: e.text.clone() },
            };
            ratatui_code_editor::code::Edit { kind }
        }).collect::<Vec<_>>();

        let last_change = changed_ranges.last().unwrap();

        // create an edit batch with fallback
        let editbatch = EditBatch {
            edits: editor_edits,
            state_before: Some(EditState {
                offset: self.editor.get_cursor(), selection: self.editor.get_selection(),
            }),
            state_after: Some(EditState {
                offset: last_change.end, selection: self.editor.get_selection(),
            }),
        };

        // apply edits to editor
        self.editor.apply_batch(&editbatch);

        // move cursor to the last of changed range
        self.editor.set_cursor(last_change.end);

        // map changed ranges to ratatui-code-editor mark type
        let marks = changed_ranges.iter().map(|r| {
            match r.kind {
                ChangedRangeKind::Insert => (r.start, r.end, COLOR_INSERT),
                ChangedRangeKind::Delete => (r.start, r.end, COLOR_DELETE),
            }
        }).collect::<Vec<_>>();

        // apply marks to editor
        self.editor.set_marks(marks);

        Ok(())
    }

}

fn is_save_pressed(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) &&
        key.code == KeyCode::Char('s')
}

fn is_autocomplete_pressed(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) &&
        key.code == KeyCode::Char(' ')
}

fn is_quit_pressed(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) &&
        key.code == KeyCode::Char('q')
}

fn save_to_file(content: &str, path: &str) -> Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(content.as_bytes())?;
    Ok(())
}

fn is_focused(mouse: &MouseEvent,area: Rect) -> bool {
    let x = mouse.column;
    let y = mouse.row;
    if rect_contains(area, x, y) { true } else { false }
}

fn rect_contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x &&
    x < rect.x + rect.width &&
    y >= rect.y &&
    y < rect.y + rect.height
}
