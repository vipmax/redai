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
use ratatui_code_editor::history::{EditKind};
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

use crate::diff::compute_changed_ranges_normalized;
use crate::coder::Coder;
use crate::llm::LlmClient;
use crate::diff::ChangedRangeKind;
use crate::diff::EditKind::*;
use crate::diff::Edit;
use crate::utils::{abs_file};
use crate::watcher::FsWatcher;
use crate::search::{SearchPanel, SearchAction, SearchMode};
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
}

pub type Theme = Vec<(&'static str, &'static str)>;

pub struct App {
    editor: Editor,
    editor_area: Rect,
    filename: String,
    fallback: Option<Fallback>,
    //static lifetime
    theme: Theme,
    quit: bool,
    split_ratio: usize,
    is_resizing: bool,
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
}

impl App {
    pub fn new(
        language: &str, content: &str,
        filename: &str, llm_client: LlmClient
    ) -> Self {
        let root_path = std::env::current_dir().unwrap();
        let theme = vesper();
        let items = build_initial_tree_items(&root_path, &theme);
        let editor = Editor::new(language, content, theme.clone());
        let mut coder = Coder::new(llm_client);
        coder.update(&PathBuf::from(filename), &content);
        let (tx, rx) = mpsc::channel(1);
        let tree_focused = if filename.is_empty() { true } else { false };
        let watcher = FsWatcher::new();

        let mut tree_state = TreeState::default();

        if let Some(item) = items.first() {
            tree_state.open(vec![item.identifier().clone()]);
        }

        Self {
            quit: false,
            editor,
            editor_area: Rect::default(),
            filename: filename.to_string(),
            fallback: None,
            theme: theme,
            coder: Arc::new(Mutex::new(coder)),
            autocomplete_handle: None,
            autocomplete_tx: tx,
            autocomplete_rx: rx,
            split_ratio: 30,
            is_resizing: false,
            left_panel_focused: tree_focused,
            tree_area: Rect::default(),
            tree_state: tree_state,
            tree_items: items,
            watcher,
            self_update: false,
            search_panel: SearchPanel::new(),
            left_panel_mode: LeftPanelMode::Tree,
        }
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
            }
        }

        Ok(())
    }

    fn render(&mut self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(self.split_ratio as u16),
                Constraint::Percentage((100 - self.split_ratio) as u16),
            ])
            .split(frame.area());

        self.tree_area = chunks[0];
        self.editor_area = chunks[1];

        // Render left panel based on mode
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

        if self.filename.is_empty() {
            let welcome = create_welcome_widget();
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
                    self.left_panel_focused = !self.left_panel_focused;
                    return Ok(());
                }
            }
            Event::Mouse(mouse) => {
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
                self.editor.paste(paste)?;
            }
            Event::Key(key) => {
                // Handle search activation
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('f') {
                    self.fallback = Some(Fallback {
                        filename: self.filename.clone(),
                        cursor: self.editor.get_cursor(),
                        selection: self.editor.get_selection(),
                        offsets: (self.editor.get_offset_y(), self.editor.get_offset_x()),
                    });
                    self.search_panel.activate(SearchMode::Search);
                    self.left_panel_mode = LeftPanelMode::Search;
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
                    });
                    self.search_panel.activate(SearchMode::GlobalSearch);
                    self.left_panel_mode = LeftPanelMode::Search;
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
                    self.editor.handle_undo();
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
                            self.editor.handle_undo();
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
        if let Event::Key(key) = event {
            if key.modifiers.contains(KeyModifiers::CONTROL) &&
                (key.code == KeyCode::Char('g') || key.code == KeyCode::Char('f')) {
                
                if key.code == KeyCode::Char('f') {
                    self.search_panel.mode = SearchMode::Search;
                } 
                if key.code == KeyCode::Char('g') {
                    self.search_panel.mode = SearchMode::GlobalSearch;
                }
                if !self.search_panel.query.is_empty() {
                    if self.search_panel.mode == SearchMode::GlobalSearch {
                        let root_path = std::env::current_dir().unwrap();
                        self.search_panel.global_search(&root_path);
                    } else {
                        let content = self.editor.get_content();
                        self.search_panel.search(&content);
                    }
                }
                return Ok(());
            }

            let action = self.search_panel.handle_input(*key);
            match action {
                SearchAction::UpdateSearch => {
                    if self.search_panel.mode == SearchMode::GlobalSearch {
                        // global search by files in current directory
                        let root_path = std::env::current_dir().unwrap();
                        self.search_panel.global_search(&root_path);
                    } else {
                        // local search in current file
                        let content = self.editor.get_content();
                        self.search_panel.search(&content);
                    }
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

                    self.left_panel_mode = LeftPanelMode::Tree;
                    let cursor_pos = result.match_start;
                    self.editor.set_cursor(cursor_pos);
                    self.editor.focus(&self.editor_area);
                    self.left_panel_focused = false;
                    self.editor.remove_marks();
                    self.fallback = None;
                }
                SearchAction::Close => {
                    if let Some(fallback) = self.fallback.take() {
                        let _ = self.open_file(&fallback.filename).await;
                        self.editor.set_cursor(fallback.cursor);
                        if let Some(selection) = fallback.selection {
                            self.editor.set_selection(selection);
                        }
                        self.editor.set_offset_y(fallback.offsets.0);
                        self.editor.set_offset_x(fallback.offsets.1);
                        self.editor.focus(&self.editor_area);
                    }
                    self.left_panel_mode = LeftPanelMode::Tree;
                    self.left_panel_focused = false;
                }
                SearchAction::None => {}
            }
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
        if self.filename == filename { return Ok(()) }
        let path = Path::new(&filename);
        if path.is_dir() { return Ok(()) }
        let theme = vesper();
        let mut language = get_lang(&filename);
        if language == "unknown" {
            language = "shell".to_string();
        }
        let content = fs::read_to_string(&filename)?;
        self.editor = Editor::new(&language, &content, theme);
        self.filename = filename.to_string();
        let mut coder = self.coder.lock().await;
        coder.update(&PathBuf::from(filename), &content);
        self.watcher.add(path)?;
        self.left_panel_focused = false;
        Ok(())
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
        if let Ok(edits) = result {
            self.apply_edits(edits).await?
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
            ratatui_code_editor::history::Edit { kind }
        }).collect::<Vec<_>>();

        // apply edits to editor
        self.editor.apply_edits(&editor_edits);

        // move cursor to the last of changed range
        if let Some(r) = changed_ranges.last() {
            self.editor.set_cursor(r.end);
        }

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

fn create_welcome_widget<'a>() -> ratatui::widgets::Paragraph<'a> {
    Paragraph::new(" Welcome to redai!")
        .style(Style::default().fg(Color::Reset))
        .wrap(Wrap { trim: false })
}
