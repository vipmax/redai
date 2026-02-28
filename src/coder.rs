use crate::diff::{Edit, compute_text_edits};
use crate::llm::LlmClient;
use crate::prompts::*;
use crate::tracker::Tracker;
use crate::utils::{byte_to_point, offset_to_byte};
use anyhow::{Result, anyhow};
use log::debug;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;

pub struct Coder {
    llm: LlmClient,
    file_trackers: HashMap<PathBuf, Tracker>,
}

impl Coder {
    pub fn new(llm: LlmClient) -> Self {
        Self {
            llm,
            file_trackers: HashMap::new(),
        }
    }

    pub async fn autocomplete(
        &self,
        original: &str,
        _path: &str,
        cursor: usize,
    ) -> Result<Vec<Edit>> {
        let cursor_byte = offset_to_byte(cursor, original);

        let context = self.build_context(original, cursor_byte, 3)?;
        debug!("context {:?}", context);

        let big_context = self.build_context(original, cursor_byte, 1000)?;

        let recent_edits_summary = self.summarize_recent_edits_for_last_files(3);
        debug!("recent_edits_context {:?}", recent_edits_summary);

        let messages = vec![
            json!({ "role": "system", "content": SYSTEM_PROMPT }),
            json!({ "role": "user", "content": format!("Big context:\n{}", big_context.0) }),
            json!({ "role": "user", "content": format!("Small context:\n{}", context.0) }),
            json!({ "role": "user", "content": format!("Recent user activity:\n{}", recent_edits_summary) }),
            json!({ "role": "user", "content": REMINDER }),
        ];

        let response = self.llm.chat(messages).await?;
        debug!("response {}", response);

        let patch = self.parse_patch(&response, cursor)?;
        debug!("patch {:?}", patch);

        let (start, search, replace) = patch;

        let edits = compute_text_edits(&search, &replace);
        debug!("edits {:?}", edits);

        let mut edits = edits
            .iter()
            .map(|edit| {
                let s = edit.start + start;
                let e = edit.end + start;
                Edit {
                    start: s,
                    end: e,
                    text: edit.text.clone(),
                    kind: edit.kind.clone(),
                }
            })
            .collect::<Vec<_>>();

        edits.sort_by(|a, b| b.start.cmp(&a.start));

        Ok(edits)
    }

    fn build_context(
        &self,
        original: &str,
        cursor_byte: usize,
        context_lines: usize,
    ) -> Result<(String, usize)> {
        let mut original = original.to_string();
        original.insert_str(cursor_byte, CTOKEN);

        let lines: Vec<&str> = original.lines().collect();

        let (line, _col) = byte_to_point(cursor_byte, &original);
        let cursor_line = line;
        let max_row = lines.len().saturating_sub(1);

        let mut before = context_lines;
        let mut after = context_lines;

        if cursor_line < context_lines {
            after += context_lines - cursor_line;
        } else if cursor_line + context_lines > max_row {
            before += (cursor_line + context_lines) - max_row;
        }

        let start_line = cursor_line.saturating_sub(before);
        let end_line = (cursor_line + after).min(max_row);

        let context = lines[start_line..=end_line].join("\n");

        let cursor_relative = context
            .find(CTOKEN)
            .ok_or_else(|| anyhow!("CTOKEN not found in context"))?;

        let start = cursor_byte - cursor_relative;

        Ok((context, start))
    }

    fn parse_patch(&self, patch: &str, cursor: usize) -> Result<(usize, String, String)> {
        let search_start = patch
            .find(STOKEN)
            .ok_or_else(|| anyhow!("Invalid patch format: missing {}", STOKEN))?;
        let replace_divider = patch
            .find(DTOKEN)
            .ok_or_else(|| anyhow!("Invalid patch format: missing {}", DTOKEN))?;
        let _replace_end = patch
            .find(RTOKEN)
            .ok_or_else(|| anyhow!("Invalid patch format: missing {}", RTOKEN))?;

        let search = &patch[search_start + STOKEN.len()..replace_divider];

        let cursor_pos = search
            .find(CTOKEN)
            .ok_or_else(|| anyhow::anyhow!("Invalid patch format: missing {}", CTOKEN))?;
        let before = &search[..cursor_pos];

        let search = search.replace(CTOKEN, "");

        let replace = &patch[replace_divider + DTOKEN.len()..];
        let replace = replace.replace(RTOKEN, "").replace(CTOKEN, "");

        let before_chars_len = before.chars().count();
        let start = cursor.saturating_sub(before_chars_len);

        Ok((start, search, replace))
    }

    pub fn update(&mut self, path: &PathBuf, content: &str) {
        let tracker = self
            .file_trackers
            .entry(path.clone())
            .or_insert_with(|| Tracker::new(content.to_string()));

        tracker.update(content.to_string());
    }

    pub fn last_modified_files(&self, n: usize) -> Vec<PathBuf> {
        let mut files_with_latest: Vec<_> = self
            .file_trackers
            .iter()
            .filter_map(|(path, tracker)| {
                tracker
                    .snapshots()
                    .last()
                    .map(|snap| (path, snap.timestamp))
            })
            .collect();

        files_with_latest.sort_by_key(|&(_, ts)| std::cmp::Reverse(ts));

        files_with_latest
            .into_iter()
            .take(n)
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub fn summarize_recent_edits_for_last_files(&self, n: usize) -> String {
        let last_files = self.last_modified_files(n);

        let mut summary = String::new();
        for file_path in last_files {
            if let Some(tracker) = self.file_trackers.get(&file_path) {
                let changes = tracker.summarize_recent_edits();
                if !changes.trim().is_empty() {
                    summary.push_str(&format!(
                        "{} changes:\n{}\n\n",
                        file_path.to_string_lossy(),
                        changes,
                    ));
                }
            }
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    #[test]
    fn test_build_context_basic() {
        let code = indoc! {r#"
fn main() {
    for i in 0..5 {
        println!("Current value: {}", );
    }
}
        "#};

        let cursor = 70;

        let coder = Coder::new(LlmClient::new("", "", ""));

        let context = coder.build_context(&code, cursor, 1).unwrap();

        println!("context:\n {:?}", context);

        assert!(context.0.contains(CTOKEN));
        assert!(context.1 == 12);
    }

    #[test]
    fn test_parse_patch() -> anyhow::Result<()> {
        let coder = Coder::new(LlmClient::new("", "", ""));

        let patch = "<|SEARCH|>let <|cursor|> = 10;<|DIVIDE|>let x = 10;<|REPLACE|>";
        let start_pos = 0;

        let parsed = coder.parse_patch(patch, start_pos)?;
        let (start, search, replace) = parsed;
        assert_eq!(start, start_pos);
        assert_eq!(search, "let  = 10;");
        assert_eq!(replace, "let x = 10;");

        Ok(())
    }

    #[test]
    fn test_parse_patch_unicode() -> anyhow::Result<()> {
        let coder = Coder::new(LlmClient::new("", "", ""));

        let patch = r#"<|SEARCH|>let <|cursor|> = "йцук";<|DIVIDE|>let x = "йцук";<|REPLACE|>"#;
        let start_pos = 0;

        let parsed = coder.parse_patch(patch, start_pos)?;
        let (start, search, replace) = parsed;
        assert_eq!(start, start_pos);
        assert_eq!(search, "let  = \"йцук\";");
        assert_eq!(replace, "let x = \"йцук\";");

        Ok(())
    }
}
