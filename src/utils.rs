use crate::app::Theme;
use crossterm::event::MouseEvent;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui_code_editor::utils::rgb;

pub const DEFAULT_IGNORE_DIRS: &[&str] = &[
    // Python
    "__pycache__",
    ".pytest_cache",
];

// Directories that should be ignored during search (even if shown in tree)
pub const SEARCH_IGNORE_DIRS: &[&str] = &[
    // Build artifacts and output directories
    "target",
    "node_modules",
    "dist",
    "build",
    "out",
    "bin",
    "obj",
    // Version control
    ".git",
    // Python
    ".venv",
    "venv",
    "env",
    ".mypy_cache",
    ".tox",
    // JavaScript/TypeScript
    ".next",
    ".nuxt",
    ".output",
    "coverage",
    // Java
    ".gradle",
    ".m2",
    "classes",
    // System files
    ".DS_Store",
];

pub const DEFAULT_IGNORE_FILES: &[&str] = &[
    // System files
    ".DS_Store",
    "Thumbs.db",
    "desktop.ini",
    // Images and video
    "*.png",
    "*.jpg",
    "*.jpeg",
    "*.gif",
    "*.bmp",
    "*.tiff",
    "*.webp",
    "*.svg",
    "*.ico",
    "*.mp4",
    "*.mov",
    "*.avi",
    "*.mkv",
    "*.webm",
    "*.flv",
    "*.wmv",
];

/// Get ignore directories with support for environment variable extension
pub fn get_ignore_dirs() -> Vec<&'static str> {
    let mut dirs = DEFAULT_IGNORE_DIRS.to_vec();

    if let Ok(extra_dirs) = std::env::var("REDAI_IGNORE_DIRS") {
        for dir in extra_dirs.split(',') {
            let dir = dir.trim();
            if !dir.is_empty() {
                // We need to leak the string to make it 'static
                // This is acceptable since ignore patterns are typically set once
                dirs.push(Box::leak(dir.to_string().into_boxed_str()));
            }
        }
    }

    dirs
}

/// Get ignore files with support for environment variable extension
pub fn get_ignore_files() -> Vec<&'static str> {
    let mut files = DEFAULT_IGNORE_FILES.to_vec();

    if let Ok(extra_files) = std::env::var("REDAI_IGNORE_FILES") {
        for file in extra_files.split(',') {
            let file = file.trim();
            if !file.is_empty() {
                // We need to leak the string to make it 'static
                // This is acceptable since ignore patterns are typically set once
                files.push(Box::leak(file.to_string().into_boxed_str()));
            }
        }
    }

    files
}

/// Checks if any part of the path matches an ignored directory
pub fn is_ignored_dir(path: &std::path::PathBuf) -> bool {
    let ignore_dirs = get_ignore_dirs();
    path.iter()
        .any(|p| ignore_dirs.contains(&p.to_string_lossy().as_ref()))
}

/// Checks if a file should be ignored based on its name or extension
pub fn is_ignored_file(file_name: &str) -> bool {
    let ignore_files = get_ignore_files();
    ignore_files.iter().any(|&pattern| {
        if pattern.starts_with('*') && pattern.len() > 1 {
            // Handle wildcard patterns like "*.log"
            let extension = &pattern[1..];
            file_name.ends_with(extension)
        } else {
            // Exact match
            file_name == pattern
        }
    })
}

/// Checks if a path should be ignored (either directory or file)
pub fn is_ignored_path(path: &std::path::PathBuf) -> bool {
    // Check if any directory in the path should be ignored
    if is_ignored_dir(path) {
        return true;
    }

    // Check if the file itself should be ignored
    if let Some(file_name) = path.file_name() {
        if let Some(file_name_str) = file_name.to_str() {
            return is_ignored_file(file_name_str);
        }
    }

    false
}

/// Checks if a directory should be ignored during search
pub fn is_search_ignored_dir(path: &std::path::Path) -> bool {
    path.iter()
        .any(|p| SEARCH_IGNORE_DIRS.contains(&p.to_string_lossy().as_ref()))
}

/// Checks if a file should be skipped during search (too large or binary)
pub fn should_skip_file_for_search(path: &std::path::Path) -> bool {
    // Check file size (skip files larger than 10MB)
    const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10MB

    if let Ok(metadata) = std::fs::metadata(path) {
        if metadata.len() > MAX_FILE_SIZE {
            return true;
        }
    }

    // Check if file is binary by reading first 512 bytes
    if let Ok(mut file) = std::fs::File::open(path) {
        use std::io::Read;
        let mut buffer = vec![0u8; 512];
        if let Ok(size) = file.read(&mut buffer) {
            // Check for null bytes or high percentage of non-printable characters
            let null_count = buffer[..size].iter().filter(|&&b| b == 0).count();
            if null_count > 0 {
                return true; // Binary file
            }

            // Check if it's valid UTF-8
            if std::str::from_utf8(&buffer[..size]).is_err() {
                // If not valid UTF-8, check if it's mostly non-printable
                let non_printable = buffer[..size]
                    .iter()
                    .filter(|&&b| !(b >= 32 && b < 127) && b != 9 && b != 10 && b != 13)
                    .count();
                if non_printable as f64 / size as f64 > 0.3 {
                    return true; // Likely binary
                }
            }
        }
    }

    false
}

/// Returns the absolute path of the input
pub fn abs_file(input: &str) -> String {
    let srcdir = std::path::PathBuf::from(input);
    let c = std::fs::canonicalize(&srcdir).unwrap();
    c.to_string_lossy().to_string()
}

/// Returns relative path for current dor
pub fn relative_to_current_dir(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let current_dir = std::env::current_dir().ok()?;
    path.strip_prefix(&current_dir)
        .ok()
        .map(|p| p.to_path_buf())
}

/// Converts a byte index to a line and column number
pub fn byte_to_point(b: usize, s: &str) -> (usize, usize) {
    let mut line = 0;
    let mut col = 0;
    let mut byte_pos = 0;

    for ch in s.chars() {
        let ch_len = ch.len_utf8();
        if byte_pos + ch_len > b {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
        byte_pos += ch_len;
    }

    (line, col)
}

pub fn offset_to_byte(o: usize, s: &str) -> usize {
    let mut byte_index = 0;
    let mut chars = s.chars();

    for _ in 0..o {
        if let Some(c) = chars.next() {
            let l = c.len_utf8();
            byte_index += l;
        } else {
            panic!("Out of bounds byte index");
        }
    }

    byte_index
}

pub fn get_line(line_number: usize, text: &str) -> &str {
    let mut line_start = 0;
    let mut current_line = 0;

    for (i, c) in text.char_indices() {
        if c == '\n' {
            if current_line == line_number {
                return &text[line_start..i];
            }
            current_line += 1;
            line_start = i + 1;
        }
    }

    if current_line == line_number {
        return &text[line_start..];
    }

    ""
}

pub fn find_color(theme: &Theme, key: &str) -> Option<Color> {
    theme.iter().find(|(k, _)| *k == key).map(|(_, v)| {
        let (r, g, b) = rgb(*v);
        Color::Rgb(r, g, b)
    })
}

pub fn is_focused(mouse: &MouseEvent, area: Rect) -> bool {
    mouse.column >= area.x
        && mouse.column < area.x + area.width
        && mouse.row >= area.y
        && mouse.row < area.y + area.height
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_byte_to_point_ascii() {
        let text = "hello\nworld";
        assert_eq!(byte_to_point(6, text), (1, 0));
        assert_eq!(byte_to_point(8, text), (1, 2));
    }

    #[test]
    fn test_byte_to_point_russian() {
        let text = "привет\nмир";
        assert_eq!(byte_to_point(13, text), (1, 0));
        assert_eq!(byte_to_point(15, text), (1, 1));
        assert_eq!(byte_to_point(6, text), (0, 3));
    }

    #[test]
    fn test_is_ignored_dir() {
        let path = PathBuf::from("src/__pycache__/package");
        assert!(is_ignored_dir(&path));

        let path = PathBuf::from("src/.pytest_cache/module");
        assert!(is_ignored_dir(&path));

        let path = PathBuf::from("src/main.rs");
        assert!(!is_ignored_dir(&path));

        let path = PathBuf::from("src/node_modules/package");
        assert!(!is_ignored_dir(&path)); // No longer ignored in tree
    }

    #[test]
    fn test_is_ignored_file() {
        assert!(is_ignored_file(".DS_Store"));
        assert!(is_ignored_file("Thumbs.db"));
        assert!(is_ignored_file("desktop.ini"));
        assert!(is_ignored_file("image.png"));
        assert!(is_ignored_file("photo.jpg"));
        assert!(is_ignored_file("video.mp4"));

        assert!(!is_ignored_file("main.rs"));
        assert!(!is_ignored_file("index.html"));
        assert!(!is_ignored_file("config.json"));
        assert!(!is_ignored_file("package-lock.json")); // No longer ignored
    }

    #[test]
    fn test_is_ignored_path() {
        let path = PathBuf::from("src/__pycache__/module.pyc");
        assert!(is_ignored_path(&path));

        let path = PathBuf::from("src/.DS_Store");
        assert!(is_ignored_path(&path));

        let path = PathBuf::from("src/main.rs");
        assert!(!is_ignored_path(&path));

        let path = PathBuf::from("image.png");
        assert!(is_ignored_path(&path));

        let path = PathBuf::from("src/node_modules/package.json");
        assert!(!is_ignored_path(&path)); // No longer ignored in tree
    }

    #[test]
    fn test_is_search_ignored_dir() {
        let path = PathBuf::from("target/debug");
        assert!(is_search_ignored_dir(&path.as_path()));

        let path = PathBuf::from("node_modules/package");
        assert!(is_search_ignored_dir(&path.as_path()));

        let path = PathBuf::from(".git/config");
        assert!(is_search_ignored_dir(&path.as_path()));

        let path = PathBuf::from("src/main.rs");
        assert!(!is_search_ignored_dir(&path.as_path()));
    }

    #[test]
    fn test_get_line() {
        let text = "\
first line
second line
third line";

        assert_eq!(get_line(0, text), "first line");
        assert_eq!(get_line(1, text), "second line");
        assert_eq!(get_line(2, text), "third line");
        assert_eq!(get_line(3, text), ""); // out of bounds
    }

    #[test]
    fn test_get_line_empty_text() {
        let text = "";
        assert_eq!(get_line(0, text), "");
        assert_eq!(get_line(1, text), "");
    }
}
