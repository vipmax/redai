use crossterm::event::{
    Event, KeyCode, KeyEvent,
    KeyModifiers, MouseEvent, MouseEventKind
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use rayon::prelude::*;
use regex::RegexBuilder;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::utils::*;

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub line: usize,
    pub column: usize,
    pub match_start: usize,
    pub match_end: usize,
    pub line_content: String,
    pub file_path: Option<String>,
}

#[derive(Clone, Debug)]
pub enum SearchUpdate {
    /// Search progress: processed files out of total
    Progress { processed: usize, total: usize },
    /// New results found
    Results(Vec<SearchResult>),
    /// Search finished
    Finished {
        files_processed: usize,
        duration: std::time::Duration,
    },
}

pub enum SearchAction {
    None,
    Close,
    UpdateSearch,
    Clear,
    JumpTo(SearchResult),
    JumpToAndExit(SearchResult),
}

#[derive(PartialEq)]
pub enum SearchMode {
    Search,
    GlobalSearch,
}

pub struct SearchPanel {
    pub active: bool,
    pub query: String,
    pub case_sensitive: bool,
    pub regex_mode: bool,
    pub results: Vec<SearchResult>,
    pub scroll_offset: usize,
    pub selected: Option<usize>,
    pub mode: SearchMode,
    pub search_time: Option<std::time::Duration>,
    pub files_processed: Option<usize>,
    pub search_in_progress: bool,
    pub search_progress: Option<(usize, usize)>,
    rx: mpsc::UnboundedReceiver<SearchUpdate>,
    tx: mpsc::UnboundedSender<SearchUpdate>,
    handle: Option<JoinHandle<()>>,
}

impl SearchPanel {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            active: false,
            query: String::new(),
            case_sensitive: false,
            regex_mode: false,
            results: Vec::new(),
            scroll_offset: 0,
            selected: None,
            mode: SearchMode::Search,
            search_time: None,
            files_processed: None,
            search_in_progress: false,
            search_progress: None,
            rx,
            tx,
            handle: None,
        }
    }

    pub async fn recv(&mut self) -> Option<SearchUpdate> {
        self.rx.recv().await
    }

    /// Apply a search update to this panel's state
    pub fn apply_update(&mut self, update: SearchUpdate) {
        match update {
            SearchUpdate::Progress { processed, total } => {
                self.search_progress = Some((processed, total));
            }
            SearchUpdate::Results(new_results) => {
                self.results.extend(new_results);
                if !self.results.is_empty() && self.selected.is_none() {
                    self.selected = Some(0);
                }
            }
            SearchUpdate::Finished {
                files_processed,
                duration,
            } => {
                if self.handle.is_some() {
                    self.search_in_progress = false;
                    self.search_time = Some(duration);
                    self.files_processed = Some(files_processed);
                    self.search_progress = None;
                }
                self.handle = None;
            }
        }
    }

    /// Cancel an in-progress search
    pub fn cancel(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
            self.search_in_progress = false;
            self.search_progress = None;
        }
    }

    /// Start a global search, managing the handle internally
    pub fn start_global_search(&mut self, root_path: std::path::PathBuf) {
        self.cancel();
        self.results.clear();
        self.selected = None;
        self.scroll_offset = 0;
        self.search_in_progress = true;
        self.search_progress = None;

        self.handle = Some(Self::spawn_global_search(
            root_path,
            self.query.clone(),
            self.case_sensitive,
            self.regex_mode,
            self.tx.clone(),
        ));
    }

    pub fn activate(&mut self, mode: SearchMode) {
        self.active = true;
        self.mode = mode;
    }

    pub fn deactivate(&mut self) {
        self.active = false;
        self.query.clear();
        self.results.clear();
        self.selected = None;
        self.scroll_offset = 0;
        self.search_time = None;
        self.files_processed = None;
        self.search_in_progress = false;
        self.search_progress = None;
    }

    pub fn handle_event(&mut self, event: &Event, area: Rect) -> SearchAction {
        match event {
            Event::Paste(paste) => {
                self.query.push_str(&paste.to_string());
                if self.mode == SearchMode::GlobalSearch {
                    SearchAction::None
                } else {
                    SearchAction::UpdateSearch
                }
            }
            Event::Key(key) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    let mode = match key.code {
                        KeyCode::Char('f') => Some(SearchMode::Search),
                        KeyCode::Char('g') => Some(SearchMode::GlobalSearch),
                        _ => None,
                    };
                    if let Some(new_mode) = mode {
                        self.mode = new_mode;
                        if !self.query.is_empty() {
                            return SearchAction::UpdateSearch;
                        }
                        return SearchAction::None;
                    }
                }
                self.handle_input(*key, area)
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::Down(_) => self.handle_mouse_click(mouse, area),
                MouseEventKind::ScrollDown => {
                    self.scroll_down(area);
                    SearchAction::None
                }
                MouseEventKind::ScrollUp => {
                    self.scroll_up();
                    SearchAction::None
                }
                _ => SearchAction::None,
            },
            _ => SearchAction::None,
        }
    }

    pub fn handle_input(&mut self, key: KeyEvent, area: Rect) -> SearchAction {
        if !self.active {
            return SearchAction::None;
        }

        match key.code {
            KeyCode::Esc => {
                self.deactivate();
                SearchAction::Close
            }
            KeyCode::Enter => {
                if let Some(selected) = self.selected {
                    if let Some(result) = self.results.get(selected) {
                        return SearchAction::JumpToAndExit(result.clone());
                    }
                }
                if self.mode == SearchMode::GlobalSearch {
                    SearchAction::UpdateSearch
                } else {
                    SearchAction::None
                }
            }
            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => match c {
                'c' => {
                    self.case_sensitive = !self.case_sensitive;
                    if self.mode == SearchMode::GlobalSearch {
                        SearchAction::None
                    } else {
                        SearchAction::UpdateSearch
                    }
                }
                'r' => {
                    self.regex_mode = !self.regex_mode;
                    if self.mode == SearchMode::GlobalSearch {
                        SearchAction::None
                    } else {
                        SearchAction::UpdateSearch
                    }
                }
                _ => SearchAction::None,
            },
            KeyCode::Char(c) => {
                self.query.push(c);
                if self.mode == SearchMode::GlobalSearch {
                    SearchAction::Clear
                } else {
                    SearchAction::UpdateSearch
                }
            }
            KeyCode::Backspace => {
                if self.mode == SearchMode::GlobalSearch {
                    self.query.pop();
                    SearchAction::Clear
                } else {
                    self.query.pop();
                    SearchAction::UpdateSearch
                }
            }
            KeyCode::Up => {
                let selected = self.selected.unwrap_or(0);
                if selected > 0 {
                    self.selected = Some(selected - 1);
                    // Adjust scroll_offset to keep selected item visible
                    let visible_height = area.height.saturating_sub(7) as usize;
                    if let Some(sel) = self.selected {
                        if sel < self.scroll_offset {
                            self.scroll_offset = sel;
                        } else if sel >= self.scroll_offset + visible_height {
                            self.scroll_offset =
                                sel.saturating_sub(visible_height.saturating_sub(1));
                        }
                    }
                    if let Some(result) = self.results.get(selected - 1) {
                        return SearchAction::JumpTo(result.clone());
                    }
                }
                SearchAction::None
            }
            KeyCode::Down => {
                let selected = self.selected.unwrap_or(0);
                if selected < self.results.len().saturating_sub(1) {
                    self.selected = Some(selected + 1);
                    // Adjust scroll_offset to keep selected item visible
                    let visible_height = area.height.saturating_sub(7) as usize;
                    if let Some(sel) = self.selected {
                        if sel < self.scroll_offset {
                            self.scroll_offset = sel;
                        } else if sel >= self.scroll_offset + visible_height {
                            self.scroll_offset =
                                sel.saturating_sub(visible_height.saturating_sub(1));
                        }
                    }
                    if let Some(result) = self.results.get(selected + 1) {
                        return SearchAction::JumpTo(result.clone());
                    }
                }
                SearchAction::None
            }
            _ => SearchAction::None,
        }
    }

    pub fn handle_mouse_click(&mut self, mouse: &MouseEvent, area: Rect) -> SearchAction {
        if !self.active {
            return SearchAction::None;
        }

        // Calculate the layout chunks to find the results list area
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Search input box
                Constraint::Length(2), // Options
                Constraint::Length(1), // Results count
                Constraint::Length(1), // Timing and file count info
                Constraint::Min(1),    // Results list
            ])
            .split(area);

        let results_list_area = chunks[4];

        // Check if click is within the results list area
        if mouse.column >= results_list_area.x
            && mouse.column < results_list_area.x + results_list_area.width
            && mouse.row >= results_list_area.y
            && mouse.row < results_list_area.y + results_list_area.height
        {
            // Calculate which item was clicked
            // The relative row within the results list area (0-based)
            let relative_row = mouse.row.saturating_sub(results_list_area.y);
            let clicked_index = self.scroll_offset + relative_row as usize;

            // Clamp to valid range
            let clicked_index = clicked_index.min(self.results.len().saturating_sub(1));

            if clicked_index < self.results.len() {
                self.selected = Some(clicked_index);
                if let Some(result) = self.results.get(clicked_index) {
                    return SearchAction::JumpTo(result.clone());
                }
            }
        }

        SearchAction::None
    }

    pub fn scroll_down(&mut self, area: Rect) {
        // Calculate visible height for results area
        // Layout: input(3) + options(2) + count(1) + timing(1) + results(rest)
        let visible_height = area.height.saturating_sub(7) as usize;
        let max_offset = self.results.len().saturating_sub(visible_height);
        if self.scroll_offset < max_offset {
            self.scroll_offset += 1;
        }
    }

    pub fn scroll_up(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset -= 1;
        }
    }

    pub fn search(&mut self, content: &str) {
        self.results.clear();
        self.selected = None;
        self.scroll_offset = 0;
        self.files_processed = None;

        if self.query.is_empty() {
            self.search_time = None;
            return;
        }

        let start = Instant::now();

        let search_query = if self.case_sensitive {
            self.query.clone()
        } else {
            self.query.to_lowercase()
        };

        let content_for_search_owned;
        let content_for_search = if self.case_sensitive {
            content
        } else {
            content_for_search_owned = content.to_lowercase();
            &content_for_search_owned
        };

        let results = Self::search_matches(
            &content,
            content_for_search,
            &search_query,
            None,
            self.case_sensitive,
            self.regex_mode,
        );

        self.results.extend(results);
        self.search_time = Some(start.elapsed());

        if !self.results.is_empty() {
            self.selected = Some(0);
        }
    }

    #[cfg(test)]
    pub fn global_search(&mut self, root_path: &std::path::Path) {
        self.results.clear();
        self.selected = None;
        self.scroll_offset = 0;

        if self.query.is_empty() {
            self.search_time = None;
            self.files_processed = None;
            return;
        }

        let start = Instant::now();

        let search_query = if self.case_sensitive {
            self.query.clone()
        } else {
            self.query.to_lowercase()
        };

        // First, collect a list of all files
        let mut files = Vec::new();
        Self::collect_files(root_path, &mut files);

        // Sort files by directory level (depth), then alphabetically
        files.sort_by(|a, b| {
            let depth_a = a.components().count();
            let depth_b = b.components().count();
            match depth_a.cmp(&depth_b) {
                std::cmp::Ordering::Equal => a.cmp(b),
                other => other,
            }
        });

        let files_count = files.len();

        // Parallel search across all files using rayon
        let case_sensitive = self.case_sensitive;
        let regex_mode = self.regex_mode;

        let all_results: Vec<SearchResult> = files
            .par_iter()
            .flat_map(|file_path| {
                Self::search_in_file(file_path, &search_query, case_sensitive, regex_mode)
            })
            .collect();

        self.results.extend(all_results);
        self.search_time = Some(start.elapsed());
        self.files_processed = Some(files_count);

        if !self.results.is_empty() {
            self.selected = Some(0);
        }
    }

    /// Spawns global search in a separate task and sends updates through a channel
    /// Returns JoinHandle for task cancellation capability
    pub fn spawn_global_search(
        root_path: std::path::PathBuf,
        query: String,
        case_sensitive: bool,
        regex_mode: bool,
        tx: mpsc::UnboundedSender<SearchUpdate>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::spawn_blocking(move || {
            let start = Instant::now();

            if query.is_empty() {
                let _ = tx.send(SearchUpdate::Finished {
                    files_processed: 0,
                    duration: start.elapsed(),
                });
                return;
            }

            let search_query = if case_sensitive {
                query.clone()
            } else {
                query.clone().to_lowercase()
            };

            // Collect list of all files
            let mut files = Vec::new();
            Self::collect_files(&root_path, &mut files);

            // Sort files by directory level (depth), then alphabetically
            files.sort_by(|a, b| {
                let depth_a = a.components().count();
                let depth_b = b.components().count();
                match depth_a.cmp(&depth_b) {
                    std::cmp::Ordering::Equal => a.cmp(b),
                    other => other,
                }
            });

            let files_count = files.len();

            // Send initial progress
            let _ = tx.send(SearchUpdate::Progress {
                processed: 0,
                total: files_count,
            });

            // Process files in batches to send intermediate results
            let batch_size = 50.max(files_count / 100); // Batches of 50 files or 1% of total
            let mut all_results = Vec::new();

            for (batch_idx, batch) in files.chunks(batch_size).enumerate() {
                // Parallel batch processing
                let batch_results: Vec<SearchResult> = batch
                    .par_iter()
                    .flat_map(|file_path| {
                        Self::search_in_file(file_path, &search_query, case_sensitive, regex_mode)
                    })
                    .collect();

                // Send new batch results before adding to the main list
                if !batch_results.is_empty() {
                    let _ = tx.send(SearchUpdate::Results(batch_results.clone()));
                }

                all_results.extend(batch_results);

                // Send progress
                let processed = ((batch_idx + 1) * batch_size).min(files_count);
                let _ = tx.send(SearchUpdate::Progress {
                    processed,
                    total: files_count,
                });
            }

            // Send final results
            let duration = start.elapsed();
            let _ = tx.send(SearchUpdate::Finished {
                files_processed: files_count,
                duration,
            });
        })
    }

    fn collect_files(root_path: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        if is_search_ignored_dir(root_path) {
            return;
        }

        if let Ok(entries) = std::fs::read_dir(root_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if is_ignored_path(&path) {
                    continue;
                }

                if path.is_dir() {
                    Self::collect_files(&path, files);
                } else if path.is_file() {
                    files.push(path);
                }
            }
        }
    }

    fn search_in_file(
        file_path: &std::path::Path,
        search_query: &str,
        case_sensitive: bool,
        regex_mode: bool,
    ) -> Vec<SearchResult> {
        // Skip files that are too large or binary
        if should_skip_file_for_search(file_path) {
            return Vec::new();
        }

        if let Ok(content) = std::fs::read_to_string(file_path) {
            let file_path_str = file_path.to_string_lossy().to_string();

            let content_for_search_owned;
            let content_for_search = if case_sensitive {
                content.as_str()
            } else {
                content_for_search_owned = content.to_lowercase();
                &content_for_search_owned
            };

            Self::search_matches(
                &content,
                content_for_search,
                search_query,
                Some(file_path_str),
                case_sensitive,
                regex_mode,
            )
        } else {
            Vec::new()
        }
    }

    fn search_matches(
        content: &str,
        content_for_search: &str,
        search_query: &str,
        file_path: Option<String>,
        case_sensitive: bool,
        regex_mode: bool,
    ) -> Vec<SearchResult> {
        if regex_mode {
            // Regex path: iterate matches on original content (Unicode-aware)
            let mut results = Vec::new();
            let regex = RegexBuilder::new(search_query)
                .case_insensitive(!case_sensitive)
                .multi_line(true)
                .unicode(true)
                .build();

            if let Ok(re) = regex {
                for m in re.find_iter(content) {
                    let match_start_byte = m.start();
                    let match_end_byte = m.end();

                    let match_start_char = content[..match_start_byte].chars().count();
                    let match_end_char = content[..match_end_byte].chars().count();

                    let point = byte_to_point(match_start_byte, &content);
                    let line = get_line(point.0, content).to_string();

                    results.push(SearchResult {
                        line: point.0,
                        column: point.1,
                        match_start: match_start_char,
                        match_end: match_end_char,
                        line_content: line,
                        file_path: file_path.clone(),
                    });
                }
            }

            results
        } else {
            // Literal find path (with optional case-insensitivity via transformed view)
            let mut results = Vec::new();
            let mut start_byte = 0;

            while let Some(pos) = content_for_search[start_byte..].find(&search_query) {
                let match_start_byte = start_byte + pos;
                let match_end_byte = match_start_byte + search_query.len();

                let match_start_char = content[..match_start_byte].chars().count();
                let match_end_char = match_start_char + search_query.chars().count();

                let point = byte_to_point(match_start_byte, &content);
                let line = get_line(point.0, content).to_string();

                results.push(SearchResult {
                    line: point.0,
                    column: point.1,
                    match_start: match_start_char,
                    match_end: match_end_char,
                    line_content: line,
                    file_path: file_path.clone(),
                });

                start_byte = match_end_byte;
            }

            results
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if !self.active {
            return;
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Search input box
                Constraint::Length(2), // Options
                Constraint::Length(1), // Results count
                Constraint::Length(1), // Timing and file count info
                Constraint::Min(1),    // Results list
            ])
            .split(area);

        // Search input box
        let search_block = Block::default()
            .title(match self.mode {
                SearchMode::Search => "Search",
                SearchMode::GlobalSearch => "Global Search",
            })
            .borders(Borders::NONE)
            .border_style(Style::default().fg(Color::Cyan));

        let search_input = Paragraph::new(self.query.as_str())
            .block(search_block)
            .style(Style::default().fg(Color::White));
        frame.render_widget(search_input, chunks[0]);

        // Options
        let options = vec![
            Span::raw("Ctrl+C: "),
            Span::styled(
                if self.case_sensitive { "Case" } else { "case" },
                Style::default().fg(if self.case_sensitive {
                    Color::Green
                } else {
                    Color::Gray
                }),
            ),
            Span::raw(" | Ctrl+R: "),
            Span::styled(
                if self.regex_mode { "Regex" } else { "regex" },
                Style::default().fg(if self.regex_mode {
                    Color::Green
                } else {
                    Color::Gray
                }),
            ),
        ];
        let options_line = Line::from(options);
        let options_para = Paragraph::new(vec![
            options_line,
            Line::from(vec![Span::raw(match self.mode {
                SearchMode::Search => "↑↓: Navigate | Enter: Jump",
                SearchMode::GlobalSearch => "↑↓: Navigate | Enter: Search",
            })]),
        ])
        .style(Style::default().fg(Color::Gray));
        frame.render_widget(options_para, chunks[1]);

        // Results count
        let results_text = if self.results.is_empty() {
            "No results".to_string()
        } else {
            let selected = self.selected.map(|i| i + 1).unwrap_or(0);
            format!("{}/{} matches", selected, self.results.len())
        };
        let results_para = Paragraph::new(results_text).style(Style::default().fg(Color::Yellow));
        frame.render_widget(results_para, chunks[2]);

        // Timing and file count info with progress
        let timing_text = if self.search_in_progress {
            if let Some((processed, total)) = self.search_progress {
                let percentage = if total > 0 {
                    (processed as f32 / total as f32 * 100.0) as u32
                } else {
                    0
                };
                format!("Searching... {}% ({}/{})", percentage, processed, total)
            } else {
                "Searching...".to_string()
            }
        } else {
            match (self.search_time, self.files_processed) {
                (Some(duration), Some(file_count)) => {
                    let millis = duration.as_millis();
                    format!("Elapsed {} ms, processed {} files", millis, file_count)
                }
                (Some(duration), None) => {
                    let millis = duration.as_millis();
                    format!("Elapsed {} ms", millis)
                }
                (None, Some(file_count)) => {
                    format!("Processed {} files", file_count)
                }
                (None, None) => String::new(),
            }
        };
        let timing_para =
            Paragraph::new(timing_text).style(Style::default().fg(if self.search_in_progress {
                Color::Yellow
            } else {
                Color::Gray
            }));
        frame.render_widget(timing_para, chunks[3]);

        // Results list area
        let results_area = chunks[4];
        let visible_height = results_area.height as usize;

        // Adjust scroll_offset to keep selected item visible
        // if let Some(selected) = self.selected {
        //     if selected < self.scroll_offset {
        //         // Selected is above visible area, scroll up
        //         self.scroll_offset = selected;
        //     } else if selected >= self.scroll_offset + visible_height {
        //         // Selected is below visible area, scroll down
        //         self.scroll_offset = selected.saturating_sub(visible_height.saturating_sub(1));
        //     }
        // }

        // Calculate visible range: trim from top (scroll_offset) and bottom
        let start_idx = self.scroll_offset;
        let end_idx = (self.scroll_offset + visible_height).min(self.results.len());
        let visible_results = &self.results[start_idx..end_idx];

        // Adjust selected to be relative to visible slice
        let selected_relative = self.selected.and_then(|sel| {
            if sel >= start_idx && sel < end_idx {
                Some(sel - start_idx)
            } else {
                None
            }
        });

        // Render title
        let title_block = Block::default().borders(Borders::NONE).title("Results");
        frame.render_widget(title_block, results_area);

        // Render each visible result
        for (i, result) in visible_results.iter().enumerate() {
            let y = results_area.y + i as u16;
            if y >= results_area.y + results_area.height {
                break;
            }

            let is_selected = selected_relative == Some(i);
            let style = if is_selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let max_width = results_area.width as usize;
            let line = if let Some(file_path) = &result.file_path {
                // global search

                let relative_path = relative_to_current_dir(std::path::Path::new(file_path))
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();

                let position = format!(
                    "{}:{}:{}",
                    relative_path,
                    result.line + 1,
                    result.column + 1
                );
                let mut content = result.line_content.trim().to_string();

                // Calculate available width for content (position + space)
                let position_len = position.len();
                let available_width = max_width.saturating_sub(position_len + 1);

                // Truncate content if needed (safe UTF-8 truncation)
                let content_char_count = content.chars().count();
                if content_char_count > available_width {
                    if available_width > 3 {
                        let truncate_to = available_width.saturating_sub(3);
                        content = content.chars().take(truncate_to).collect::<String>();
                        content.push_str("...");
                    } else {
                        content = "...".to_string();
                    }
                }

                Line::from(vec![
                    Span::styled(position, Style::default().fg(Color::Blue)),
                    Span::raw(" "),
                    Span::raw(content),
                ])
            } else {
                // local search
                let position = format!("{}:{}", result.line + 1, result.column + 1);
                let mut content = result.line_content.trim().to_string();

                // Calculate available width for content (position + space)
                let position_len = position.len();
                let available_width = max_width.saturating_sub(position_len + 1);

                // Truncate content if needed (safe UTF-8 truncation)
                let content_char_count = content.chars().count();
                if content_char_count > available_width {
                    if available_width > 3 {
                        let truncate_to = available_width.saturating_sub(3);
                        content = content.chars().take(truncate_to).collect::<String>();
                        content.push_str("...");
                    } else {
                        content = "...".to_string();
                    }
                }

                Line::from(vec![
                    Span::styled(position, Style::default().fg(Color::Blue)),
                    Span::raw(" "),
                    Span::raw(content),
                ])
            };

            let item_area = Rect {
                x: results_area.x,
                y,
                width: results_area.width,
                height: 1,
            };

            let item_para = Paragraph::new(line).style(style);
            frame.render_widget(item_para, item_area);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_simple() {
        let mut search_panel = SearchPanel::new();
        search_panel.query = "let".to_string();
        search_panel.search("let mut foo = 2;\nfoo *= 50;");

        assert_eq!(search_panel.results.len(), 1);
        assert_eq!(search_panel.results[0].line, 0);
        assert_eq!(search_panel.results[0].column, 0);
        assert_eq!(search_panel.results[0].match_start, 0);
    }

    #[test]
    fn test_search_readme() {
        let mut search_panel = SearchPanel::new();
        search_panel.query = "Terminal".to_string();
        let content = std::fs::read_to_string("README.md").unwrap();
        search_panel.search(&content);

        assert_eq!(search_panel.results.len(), 5);
    }

    #[test]
    fn test_global_search_basic() {
        use std::fs::File;
        use std::io::Write;
        use tempfile::tempdir;

        // Set up test dir and files
        let dir = tempdir().expect("failed to create temp dir");
        let file_path1 = dir.path().join("file1.txt");
        let file_path2 = dir.path().join("file2.txt");

        let mut file1 = File::create(&file_path1).unwrap();
        let mut file2 = File::create(&file_path2).unwrap();

        writeln!(file1, "foo bar global foo\nanother line").unwrap();
        writeln!(file2, "this file contains foo\nno other match.").unwrap();

        let mut search_panel = SearchPanel::new();
        search_panel.query = "foo".to_string();
        search_panel.mode = SearchMode::GlobalSearch;
        search_panel.global_search(dir.path());

        // There should be 3 matches across both files
        let matches: Vec<_> = search_panel.results.iter().collect();
        assert_eq!(matches.len(), 3);

        let paths: Vec<_> = matches
            .iter()
            .map(|r| r.file_path.clone().unwrap())
            .collect();
        assert!(paths.iter().any(|p| p.contains("file1.txt")));
        assert!(paths.iter().any(|p| p.contains("file2.txt")));

        dir.close().unwrap();
    }

    #[test]
    fn test_global_search_specific_directory_with_timing() {
        use std::path::PathBuf;
        use std::time::Instant;

        let home_dir = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        let mut rust_dir = PathBuf::from(home_dir);
        rust_dir.push("dev/rust");

        let mut search_panel = SearchPanel::new();
        search_panel.query = "upstream_monomorphization".to_string();
        search_panel.mode = SearchMode::GlobalSearch;

        let start = Instant::now();
        search_panel.global_search(&rust_dir);
        let elapsed = start.elapsed();

        println!(
            "Global search in {:?} took {:.2?} seconds and found {} results.",
            rust_dir,
            elapsed,
            search_panel.results.len()
        );

        for r in &search_panel.results {
            println!(
                "File: {:?}, Line: {}, Col: {}, line_content: {}",
                r.file_path.as_ref().unwrap_or(&"unknown".to_string()),
                r.line,
                r.column,
                r.line_content
            );
        }
    }
}
