use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui_code_editor::code::{EditBatch, EditKind, EditState};
use ratatui_code_editor::editor::Editor as CodeEditor;
use ratatui_code_editor::selection::Selection;
use ratatui_code_editor::utils::get_lang;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::coder::Coder;
use crate::diff::EditKind::*;
use crate::diff::*;
use crate::llm::LlmClient;
use crate::search::SearchMode;

const COLOR_INSERT: &str = "#02a365";
const COLOR_DELETE: &str = "#f6c99f";

#[derive(Clone, Debug)]
pub struct Fallback {
    pub filename: String,
    pub cursor: usize,
    pub offsets: (usize, usize),
    pub selection: Option<Selection>,
    pub left_panel_visible: bool,
}

pub enum EditorAction {
    None,
    Quit,
    ActivateSearch(SearchMode),
    Save,
}

pub struct Autocomplete {
    coder: Arc<Mutex<Coder>>,
    handle: Option<JoinHandle<()>>,
    tx: mpsc::Sender<Result<Vec<Edit>>>,
    rx: mpsc::Receiver<Result<Vec<Edit>>>,
}

pub struct EditorPanel {
    pub editor: CodeEditor,
    pub area: Rect,
    pub filename: String,
    pub opened: HashMap<String, CodeEditor>,
    pub fallback: Option<Fallback>,
    pub autocomplete: Autocomplete,
    pub self_update: bool,
}

impl EditorPanel {
    pub fn new(
        language: &str,
        content: &str,
        filename: &str,
        llm_client: Option<LlmClient>,
    ) -> Result<Self> {
        let theme = ratatui_code_editor::theme::vesper();
        let editor = CodeEditor::new(language, content, theme)?;

        let mut coder = Coder::new(llm_client);
        coder.update(&PathBuf::from(filename), content);
        let (tx, rx) = mpsc::channel(1);

        Ok(Self {
            editor,
            area: Rect::default(),
            filename: filename.to_string(),
            opened: HashMap::new(),
            fallback: None,
            autocomplete: Autocomplete {
                coder: Arc::new(Mutex::new(coder)),
                handle: None,
                tx,
                rx,
            },
            self_update: false,
        })
    }

    pub fn render(&self, frame: &mut Frame) {
        if self.filename.is_empty() {
            let welcome = Paragraph::new(" Welcome to redai!")
                .style(Style::default().fg(Color::Reset))
                .wrap(Wrap { trim: false });
            frame.render_widget(welcome, self.area);
        } else {
            frame.render_widget(&self.editor, self.area);
            if let Some((x, y)) = self.editor.get_visible_cursor(&self.area) {
                frame.set_cursor_position(Position::new(x, y));
            }
        }
    }

    pub fn handle_event(&mut self, event: &Event) -> EditorAction {
        match event {
            Event::Paste(paste) => {
                self.editor.apply(ratatui_code_editor::actions::InsertText {
                    text: paste.to_string(),
                });
            }
            Event::Key(key) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('f') {
                    return EditorAction::ActivateSearch(SearchMode::Search);
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('g') {
                    return EditorAction::ActivateSearch(SearchMode::GlobalSearch);
                }

                self.autocomplete.handle.take().map(|h| h.abort());
                let has_marks = self.editor.has_marks();

                if key.code == KeyCode::Esc && has_marks {
                    self.editor.remove_marks();
                    self.editor.apply(ratatui_code_editor::actions::Undo {});
                } else if is_quit_pressed(*key) {
                    return EditorAction::Quit;
                } else if is_autocomplete_pressed(*key) {
                    self.spawn_autocomplete();
                } else if is_save_pressed(*key) {
                    return EditorAction::Save;
                } else {
                    let accepted = key.code == KeyCode::Tab || key.code == KeyCode::Enter;
                    if has_marks {
                        if accepted {
                            self.editor.remove_marks();
                        } else {
                            self.editor.remove_marks();
                            self.editor.apply(ratatui_code_editor::actions::Undo {});
                            let _ = self.editor.input(*key, &self.area);
                        }
                    } else {
                        let _ = self.editor.input(*key, &self.area);
                    }
                }
            }
            Event::Mouse(mouse) => {
                let _ = self.editor.mouse(*mouse, &self.area);
            }
            _ => {}
        }
        EditorAction::None
    }

    pub fn spawn_autocomplete(&mut self) {
        let tx = self.autocomplete.tx.clone();
        let content = self.editor.get_content();
        let filename = self.filename.clone();
        let cursor = self.editor.get_cursor();
        let coder = self.autocomplete.coder.clone();

        let handle = tokio::spawn(async move {
            let coder = coder.lock().await;
            let result = coder.autocomplete(&content, &filename, cursor).await;
            let _ = tx.send(result).await;
        });
        self.autocomplete.handle = Some(handle);
    }

    pub async fn recv_autocomplete(&mut self) -> Option<Result<Vec<Edit>>> {
        self.autocomplete.rx.recv().await
    }

    pub async fn handle_autocomplete(&mut self, result: Result<Vec<Edit>>) -> Result<()> {
        match result {
            Ok(edits) => self.apply_edits(edits)?,
            Err(err) => eprintln!("autocomplete error: {err:#}"),
        }
        Ok(())
    }

    pub async fn open_file(&mut self, filename: &str) -> Result<()> {
        if self.filename == filename || std::path::Path::new(filename).is_dir() {
            return Ok(());
        }

        let mut new_editor = match self.opened.remove(filename) {
            Some(ed) => ed,
            None => {
                let theme = ratatui_code_editor::theme::vesper();
                let mut lang = get_lang(filename);
                if lang == "unknown" {
                    lang = "shell".to_string();
                }
                let content = std::fs::read_to_string(filename)?;
                CodeEditor::new(&lang, &content, theme)?
            }
        };

        if !self.filename.is_empty() {
            std::mem::swap(&mut self.editor, &mut new_editor);
            self.opened.insert(self.filename.clone(), new_editor);
        } else {
            self.editor = new_editor;
        }

        let content = self.editor.get_content();
        {
            let mut coder = self.autocomplete.coder.lock().await;
            coder.update(&PathBuf::from(filename), &content);
        }

        self.filename = filename.to_string();
        Ok(())
    }

    pub async fn save(&mut self) -> Result<()> {
        let content = self.editor.get_content();
        save_to_file(&content, &self.filename)?;
        let mut coder = self.autocomplete.coder.lock().await;
        coder.update(&PathBuf::from(&self.filename), &content);
        self.self_update = true;
        Ok(())
    }

    pub async fn handle_file_change(&mut self, event: &notify::Event) -> Result<()> {
        use crate::utils::abs_file;

        match event.kind {
            notify::EventKind::Modify(notify::event::ModifyKind::Data(_)) => {
                if self.self_update {
                    self.self_update = false;
                    return Ok(());
                }
                let self_abs = abs_file(&self.filename);
                let self_path = std::path::Path::new(&self_abs);
                if event.paths.iter().any(|p| p == &self_path) {
                    let old_content = self.editor.get_content();
                    let new_content = std::fs::read_to_string(&self_path)?;
                    self.self_update = false;

                    if old_content != new_content {
                        let edits = compute_text_edits(&old_content, &new_content);
                        if edits.is_empty() {
                            self.editor.set_content(&new_content);
                            clamp_editor_state(&mut self.editor);
                        } else {
                            self.apply_external_edits(edits)?;
                        }
                        let mut coder = self.autocomplete.coder.lock().await;
                        coder.update(&PathBuf::from(&self.filename), &new_content);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_edits(&mut self, edits: Vec<Edit>) -> Result<()> {
        if edits.is_empty() {
            return Ok(());
        }

        let changed_ranges = compute_changed_ranges_normalized(&edits);
        let editor_edits = edits
            .iter()
            .map(|e| {
                let kind = match &e.kind {
                    Insert => EditKind::Insert {
                        offset: e.start,
                        text: e.text.clone(),
                    },
                    Delete => EditKind::Remove {
                        offset: e.start,
                        text: e.text.clone(),
                    },
                };
                ratatui_code_editor::code::Edit { kind }
            })
            .collect::<Vec<_>>();

        let last_change = changed_ranges.last().unwrap();
        let editbatch = EditBatch {
            edits: editor_edits,
            state_before: Some(EditState {
                offset: self.editor.get_cursor(),
                selection: self.editor.get_selection(),
            }),
            state_after: Some(EditState {
                offset: last_change.end,
                selection: self.editor.get_selection(),
            }),
        };

        self.editor.apply_batch(&editbatch);
        self.editor.set_cursor(last_change.end);

        let marks = changed_ranges
            .iter()
            .map(|r| match r.kind {
                ChangedRangeKind::Insert => (r.start, r.end, COLOR_INSERT),
                ChangedRangeKind::Delete => (r.start, r.end, COLOR_DELETE),
            })
            .collect::<Vec<_>>();
        self.editor.set_marks(marks);

        Ok(())
    }

    pub fn apply_external_edits(&mut self, edits: Vec<Edit>) -> Result<()> {
        if edits.is_empty() {
            return Ok(());
        }

        let cursor_before = self.editor.get_cursor();
        let selection_before = self.editor.get_selection();
        let editor_edits = edits
            .iter()
            .map(|e| {
                let kind = match &e.kind {
                    Insert => EditKind::Insert {
                        offset: e.start,
                        text: e.text.clone(),
                    },
                    Delete => EditKind::Remove {
                        offset: e.start,
                        text: e.text.clone(),
                    },
                };
                ratatui_code_editor::code::Edit { kind }
            })
            .collect::<Vec<_>>();

        let editbatch = EditBatch {
            edits: editor_edits,
            state_before: Some(EditState {
                offset: cursor_before,
                selection: selection_before,
            }),
            state_after: Some(EditState {
                offset: cursor_before,
                selection: selection_before,
            }),
        };

        self.editor.apply_batch(&editbatch);
        self.editor.remove_marks();
        clamp_editor_state(&mut self.editor);

        Ok(())
    }
}

fn is_save_pressed(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s')
}

fn is_autocomplete_pressed(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(' ')
}

fn is_quit_pressed(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q')
}

fn save_to_file(content: &str, path: &str) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    file.write_all(content.as_bytes())?;
    Ok(())
}

fn clamp_editor_state(editor: &mut CodeEditor) {
    let len = editor.code_ref().len_chars();
    let cursor = editor.get_cursor().min(len);
    editor.set_cursor(cursor);
    let selection = editor.get_selection().and_then(|selection| {
        let start = selection.start.min(len);
        let end = selection.end.min(len);
        let selection = Selection::new(start, end);
        (!selection.is_empty()).then_some(selection)
    });
    editor.set_selection(selection);
}
