use similar::{Algorithm, ChangeTag, TextDiff};
use unicode_segmentation::UnicodeSegmentation;

pub use ratatui_code_editor::code::{Edit, Operation};

pub fn compute_text_edits(old: &str, new: &str) -> Vec<Edit> {
    // Split into graphemes
    let old_gr: Vec<&str> = old.graphemes(true).collect();
    let new_gr: Vec<&str> = new.graphemes(true).collect();

    // Diff by graphemes
    let changes = similar::utils::diff_slices(Algorithm::Myers, &old_gr, &new_gr);

    let mut edits = Vec::new();
    let mut utf16_offset = 0usize; // position in UTF-16 units

    for (tag, slice) in changes {
        match tag {
            ChangeTag::Equal => {
                // Calculate UTF-16 length of each grapheme
                for g in slice {
                    utf16_offset += g.encode_utf16().count();
                }
            }
            ChangeTag::Delete => {
                // Combine graphemes into text
                let text = slice.concat();
                edits.push(Edit {
                    start: utf16_offset,
                    text: text.clone(),
                    operation: Operation::Remove,
                });
                // Deletion does not move offset forward
            }
            ChangeTag::Insert => {
                let text = slice.concat();
                edits.push(Edit {
                    start: utf16_offset,
                    text: text.clone(),
                    operation: Operation::Insert,
                });
                // Insertion moves offset forward
                for g in slice {
                    utf16_offset += g.encode_utf16().count();
                }
            }
        }
    }

    edits
}


// pub fn compute_text_edits(old: &str, new: &str) -> Vec<Edit> {
//     let diff = TextDiff::from_chars(old, new);
//     let mut edits: Vec<Edit> = Vec::new();

//     let mut old_pos_chars = 0;
//     let mut delete_offset_shift = 0;
//     let mut last_tag = ChangeTag::Equal;

//     for change in diff.iter_all_changes() {
//         let value = change.value();
//         let value_char_len = value.chars().count();

//         match change.tag() {
//             ChangeTag::Equal => {
//                 old_pos_chars += value_char_len;
//                 delete_offset_shift = 0;
//                 last_tag = ChangeTag::Equal;
//             }
//             ChangeTag::Delete => {
//                 if last_tag == ChangeTag::Delete {
//                     if let Some(last) = edits.last_mut() {
//                         if last.operation == Operation::Remove {
//                             last.text.push_str(value);
//                             old_pos_chars += value_char_len;
//                             delete_offset_shift += value_char_len;
//                             continue;
//                         }
//                     }
//                 }
//                 edits.push(Edit {
//                     start: old_pos_chars - delete_offset_shift,
//                     text: value.to_string(),
//                     operation: Operation::Remove,
//                 });
//                 old_pos_chars += value_char_len;
//                 delete_offset_shift += value_char_len;
//                 last_tag = ChangeTag::Delete;
//             }
//             ChangeTag::Insert => {
//                 let start = old_pos_chars - delete_offset_shift;
//                 if last_tag == ChangeTag::Insert {
//                     if let Some(last) = edits.last_mut() {
//                         if last.operation == Operation::Insert && last.start == start {
//                             last.text.push_str(value);
//                             continue;
//                         }
//                     }
//                 }
//                 edits.push(Edit {
//                     start,
//                     text: value.to_string(),
//                     operation: Operation::Insert,
//                 });
//                 last_tag = ChangeTag::Insert;
//             }
//         }
//     }

//     edits
// }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedRange {
    pub start: usize,
    pub end: usize,
    pub kind: ChangedRangeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangedRangeKind {
    Insert,
    Delete,
}

pub fn compute_changed_ranges_normalized(edits: &[Edit]) -> Vec<ChangedRange> {
    let mut sorted_edits = edits.to_vec();
    sorted_edits.sort_by_key(|e| e.start);

    let mut changed_ranges = Vec::new();
    let mut offset_shift: i64 = 0;

    for edit in sorted_edits.iter() {
        let base_start = edit.start as i64 + offset_shift;
        if base_start < 0 {
            continue;
        }
        let start = base_start as usize;

        match edit.operation {
            Operation::Insert => {
                let text_len = edit.text.chars().count();
                let end = start + text_len;
                changed_ranges.push(ChangedRange {
                    start,
                    end,
                    kind: ChangedRangeKind::Insert,
                });
                offset_shift += text_len as i64;
            }
            Operation::Remove => {
                let removed_len = edit.text.chars().count();
                let end = start + removed_len;
                changed_ranges.push(ChangedRange {
                    start,
                    end,
                    kind: ChangedRangeKind::Delete,
                });
                offset_shift -= removed_len as i64;
            }
        }
    }

    changed_ranges
}

pub fn diff_without_unchanged(old: &str, new: &str) -> String {
    let diff = TextDiff::configure()
        .algorithm(similar::Algorithm::Myers)
        .diff_lines(old, new);

    let mut result = String::new();

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Delete => result.push_str(&format!("-{}", change)),
            ChangeTag::Insert => result.push_str(&format!("+{}", change)),
            ChangeTag::Equal => {}
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_edits_simple() {
        let before = "let mut foo = 2;\nfoo *= 50;";
        let after = "let mut foo = 5;\naaaa foo *= 50;";

        let edits = compute_text_edits(before, after);

        assert_eq!(
            edits,
            vec![
                Edit {
                    start: 14,
                    text: "2".to_string(),
                    operation: Operation::Remove
                },
                Edit {
                    start: 14,
                    text: "5".to_string(),
                    operation: Operation::Insert
                },
                Edit {
                    start: 17,
                    text: "aaaa ".to_string(),
                    operation: Operation::Insert
                },
            ]
        );
    }

    #[test]
    fn test_compute_edits_simple2() {
        let before = r#"println!("Current value: {}", );"#;
        let after = r#"println!("Current value: {}", i);"#;

        let edits = compute_text_edits(before, after);

        assert_eq!(
            edits,
            vec![Edit {
                start: 30,
                text: "i".to_string(),
                operation: Operation::Insert
            }]
        )
    }

    #[test]
    fn test_compute_edits_unicode() {
        let before = r#"println!("Current значение: {}", i);"#;
        let after = r#"println!("Current value: {}", i);"#;

        let edits = compute_text_edits(before, after);

        assert_eq!(
            edits,
            vec![
                Edit {
                    start: 18,
                    text: "значение".to_string(),
                    operation: Operation::Remove
                },
                Edit {
                    start: 18,
                    text: "value".to_string(),
                    operation: Operation::Insert
                },
            ]
        )
    }

    #[test]
    fn test_compute_edits_complex() {
        let before = "main rust here";
        let after = "fn main() {\n    println!(\"Hello, world!\");\n}";

        let edits = compute_text_edits(before, after);

        // `start` offsets are in UTF-16 code units (ratatui_code_editor contract).
        assert_eq!(
            edits,
            vec![
                Edit {
                    start: 0,
                    text: "fn ".to_string(),
                    operation: Operation::Insert
                },
                Edit {
                    start: 7,
                    text: "() {\n  ".to_string(),
                    operation: Operation::Insert
                },
                Edit {
                    start: 15,
                    text: " p".to_string(),
                    operation: Operation::Insert
                },
                Edit {
                    start: 18,
                    text: "us".to_string(),
                    operation: Operation::Remove
                },
                Edit {
                    start: 18,
                    text: "in".to_string(),
                    operation: Operation::Insert
                },
                Edit {
                    start: 21,
                    text: "ln!(\"Hello,".to_string(),
                    operation: Operation::Insert
                },
                Edit {
                    start: 33,
                    text: "he".to_string(),
                    operation: Operation::Remove
                },
                Edit {
                    start: 33,
                    text: "wo".to_string(),
                    operation: Operation::Insert
                },
                Edit {
                    start: 36,
                    text: "e".to_string(),
                    operation: Operation::Remove
                },
                Edit {
                    start: 36,
                    text: "ld!\");\n}".to_string(),
                    operation: Operation::Insert
                },
            ]
        );
    }

    #[test]
    fn test_compute_changed_ranges_normalized() {
        let before = r#"println!("Current value: {}", );"#;
        let after = r#"println!("Current value: {}", i);"#;

        let edits = compute_text_edits(before, after);
        let changed_ranges = compute_changed_ranges_normalized(&edits);

        assert_eq!(
            edits,
            vec![Edit {
                start: 30,
                text: "i".to_string(),
                operation: Operation::Insert
            }]
        );
        assert_eq!(
            changed_ranges,
            vec![ChangedRange {
                start: 30,
                end: 31,
                kind: ChangedRangeKind::Insert
            },]
        );
    }

    #[test]
    fn test_compute_changed_ranges_normalized_unicode() {
        let before = r#"println!("Current value: {}", i);"#;
        let after = r#"println!("Current значение: {}", i);"#;

        let edits = compute_text_edits(before, after);
        let changed_ranges = compute_changed_ranges_normalized(&edits);

        assert_eq!(
            edits,
            vec![
                Edit {
                    start: 18,
                    text: "value".to_string(),
                    operation: Operation::Remove
                },
                Edit {
                    start: 18,
                    text: "значение".to_string(),
                    operation: Operation::Insert
                },
            ]
        );
        assert_eq!(
            changed_ranges,
            vec![
                ChangedRange {
                    start: 18,
                    end: 23,
                    kind: ChangedRangeKind::Delete
                },
                ChangedRange {
                    start: 13,
                    end: 21,
                    kind: ChangedRangeKind::Insert
                },
            ]
        );
    }
}
