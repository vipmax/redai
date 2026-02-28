use crate::app::Theme;
use crate::utils::{find_color, is_ignored_path};
use ratatui::{style::Style, text::Span};
use std::path::Path;
use tui_tree_widget::TreeItem;

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
