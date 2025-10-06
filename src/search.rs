use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use regex::RegexBuilder;

use crate::utils::{byte_to_point, get_line, is_ignored_path};

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub line: usize,
    pub column: usize,
    pub match_start: usize,
    pub match_end: usize,
    pub line_content: String,
    pub file_path: Option<String>,
}

pub enum SearchAction {
    None,
    Close,
    UpdateSearch,
    JumpTo(SearchResult),
    JumpToAndExit(SearchResult),
}

#[derive(PartialEq)]
pub enum SearchMode {
    Search,
    GlobalSearch
}

pub struct SearchPanel {
    pub active: bool,
    pub query: String,
    pub case_sensitive: bool,
    pub regex_mode: bool,
    pub results: Vec<SearchResult>,
    pub list_state: ListState,
    pub mode: SearchMode,
}

impl SearchPanel {
    pub fn new() -> Self {
        Self {
            active: false,
            query: String::new(),
            case_sensitive: false,
            regex_mode: false,
            results: Vec::new(),
            list_state: ListState::default(),
            mode: SearchMode::Search,
        }
    }

    pub fn activate(&mut self, mode: SearchMode) {
        self.active = true;
        self.mode = mode;
    }

    pub fn deactivate(&mut self) {
        self.active = false;
        self.query.clear();
        self.results.clear();
        self.list_state.select(None);
    }

    pub fn handle_input(&mut self, key: KeyEvent) -> SearchAction {
        if !self.active {
            return SearchAction::None;
        }

        match key.code {
            KeyCode::Esc => {
                self.deactivate();
                SearchAction::Close
            }
            KeyCode::Enter => {
                if let Some(selected) = self.list_state.selected() {
                    if let Some(result) = self.results.get(selected) {
                        return SearchAction::JumpToAndExit(result.clone());
                    }
                }
                SearchAction::None
            }
            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                match c {
                    'c' => {
                        self.case_sensitive = !self.case_sensitive;
                        SearchAction::UpdateSearch
                    }
                    'r' => {
                        self.regex_mode = !self.regex_mode;
                        SearchAction::UpdateSearch
                    }
                    _ => SearchAction::None,
                }
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                SearchAction::UpdateSearch
            }
            KeyCode::Backspace => {
                self.query.pop();
                SearchAction::UpdateSearch
            }
            KeyCode::Up => {
                let selected = self.list_state.selected().unwrap_or(0);
                if selected > 0 {
                    self.list_state.select(Some(selected - 1));
                    if let Some(result) = self.results.get(selected - 1) {
                        return SearchAction::JumpTo(result.clone());
                    }
                }
                SearchAction::None
            }
            KeyCode::Down => {
                let selected = self.list_state.selected().unwrap_or(0);
                if selected < self.results.len().saturating_sub(1) {
                    self.list_state.select(Some(selected + 1));
                    if let Some(result) = self.results.get(selected + 1) {
                        return SearchAction::JumpTo(result.clone());
                    }
                }
                SearchAction::None
            }
            _ => SearchAction::None,
        }
    }

    pub fn search_matches(
        &self,
        content: &str,
        content_for_search: &str,
        search_query: &str,
        file_path: Option<String>
    ) -> Vec<SearchResult> {
        if self.regex_mode {
            // Regex path: iterate matches on original content (Unicode-aware)
            let mut results = Vec::new();
            let regex = RegexBuilder::new(search_query)
                .case_insensitive(!self.case_sensitive)
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

    pub fn search(&mut self, content: &str) {
        self.results.clear();
        self.list_state.select(None);

        if self.query.is_empty() {
            return;
        }

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

        let results = self.search_matches(&content, content_for_search, &search_query, None);

        self.results.extend(results);

        if !self.results.is_empty() {
            self.list_state.select(Some(0));
        }
    }

    pub fn global_search(&mut self, root_path: &std::path::Path) {
        self.results.clear();
        self.list_state.select(None);

        if self.query.is_empty() {
            return;
        }

        let search_query = if self.case_sensitive { self.query.clone() } else { self.query.to_lowercase() };

        if let Ok(entries) = std::fs::read_dir(root_path) {
            for entry in entries.flatten() {
                let path = entry.path();

                if path.is_dir() {
                    self.search_in_directory(&path, &search_query);
                } else if path.is_file() {
                    self.search_in_file(&path, &search_query);
                }
            }
        }

        if !self.results.is_empty() {
            self.list_state.select(Some(0));
        }
    }


    fn search_in_directory(&mut self, dir_path: &std::path::Path, search_query: &str) {
        if let Ok(entries) = std::fs::read_dir(dir_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                
                if is_ignored_path(&path) {
                    continue;
                }

                if path.is_dir() {
                    self.search_in_directory(&path, search_query);
                } else if path.is_file() {
                    self.search_in_file(&path, search_query);
                }
            }
        }
    }

    fn search_in_file(&mut self, file_path: &std::path::Path, search_query: &str) {

        if let Ok(content) = std::fs::read_to_string(file_path) {
            let file_path_str = file_path.to_string_lossy().to_string();

            let content_for_search_owned;
            let content_for_search = if self.case_sensitive {
                content.as_str()
            } else {
                content_for_search_owned = content.to_lowercase();
                &content_for_search_owned
            };

            let results = self.search_matches(&content, content_for_search, search_query, Some(file_path_str));
            self.results.extend(results);

            if !self.results.is_empty() {
                self.list_state.select(Some(0));
            }

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
                Style::default().fg(if self.case_sensitive { Color::Green } else { Color::Gray }),
            ),
            Span::raw(" | Ctrl+R: "),
            Span::styled(
                if self.regex_mode { "Regex" } else { "regex" },
                Style::default().fg(if self.regex_mode { Color::Green } else { Color::Gray }),
            ),
        ];
        let options_line = Line::from(options);
        let options_para = Paragraph::new(vec![
            options_line,
            Line::from(vec![Span::raw("↑↓: Navigate | Enter: Jump")])
        ])
        .style(Style::default().fg(Color::Gray));
        frame.render_widget(options_para, chunks[1]);

        // Results count
        let results_text = if self.results.is_empty() {
            "No results".to_string()
        } else {
            let selected = self.list_state.selected().map(|i| i + 1).unwrap_or(0);
            format!("{}/{} matches", selected, self.results.len())
        };
        let results_para = Paragraph::new(results_text)
            .style(Style::default().fg(Color::Yellow));
        frame.render_widget(results_para, chunks[2]);

        let items: Vec<ListItem> = self.results
            .iter()
            .enumerate()
            .map(|(_, result)| {
                let line = if let Some(file_path) = &result.file_path {
                    // global search
                    let file_name = std::path::Path::new(file_path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    let position = format!("{}:{}:{}", file_name, result.line + 1, result.column + 1);
                    let content = result.line_content.trim().to_string();

                    Line::from(vec![
                        Span::styled(position, Style::default().fg(Color::Blue)),
                        Span::raw(" "),
                        Span::raw(content),
                    ])
                } else {
                    // local search
                    let position = format!("{}:{}", result.line + 1, result.column + 1);
                    let content = if result.line_content.len() > 50 {
                        format!("{}...", &result.line_content[..47])
                    } else {
                        result.line_content.clone()
                    };

                    Line::from(vec![
                        Span::styled(position, Style::default().fg(Color::Blue)),
                        Span::raw(" "),
                        Span::raw(content),
                    ])
                };

                ListItem::new(line)
            })
            .collect();

        let results_list = List::new(items)
            .block(Block::default().borders(Borders::NONE).title("Results"))
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            );

        frame.render_stateful_widget(results_list, chunks[3], &mut self.list_state);
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
}
