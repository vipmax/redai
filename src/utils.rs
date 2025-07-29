pub const DEFAULT_IGNORE_DIRS: &[&str] = &[
    // Version control and IDEs
    ".git", ".idea", ".vscode", ".vim", ".netrwhist", ".vs",
    
    // Build artifacts and output directories
    "node_modules", "dist", "target", "build", "out", "bin", "obj",
    
    // Python
    "__pycache__", ".pytest_cache", ".mypy_cache", ".tox", 
    ".coverage", ".venv", "venv", "env",
    
    // JavaScript/TypeScript
    ".next", ".nuxt", ".output", "coverage", ".nyc_output",
    
    // Java
    ".gradle", ".m2", "classes",
    
    // .NET
    "packages",
    
    // Ruby
    ".bundle", "vendor",
    
    // Go
    "vendor",
    
    // System files
    ".DS_Store", "Thumbs.db", "desktop.ini",
    
    // Temporary and cache files
    "tmp", "temp", ".tmp", ".cache", "cache",
    
    // Logs
    "logs", "log",
    
    // Documentation builds
    "docs/_build", "site", "_site",
    
    // Cloud and DevOps
    ".terraform", ".terragrunt-cache", ".pulumi", 
    ".vagrant", ".docker", ".kube", ".minikube",
    ".helm", ".serverless",
    
    // CI/CD
    ".github", ".gitlab-ci", ".circleci", ".buildkite",
    ".jenkins", ".azure-pipelines",
    
    // Mobile development
    ".expo", ".expo-shared", "ios/build", "android/build",
    "android/.gradle", "ios/Pods", "ios/DerivedData",
    ".flutter-plugins", ".flutter-plugins-dependencies",
    
    // Game development
    "Library", "Temp", "Logs", "MemoryCaptures",
    "Builds", "UserSettings",
    
    // Additional languages
    ".stack-work", ".cabal-sandbox", // Haskell
    "_build", ".merlin", // OCaml
    ".eunit", ".rebar", ".rebar3", // Erlang
    ".mix", "deps", // Elixir
    ".dart_tool", // Dart
    ".pio", ".platformio", // PlatformIO
    
    // Scientific computing
    ".ipynb_checkpoints", ".spyderproject", ".spyproject",
    ".RData", ".Rhistory", ".Rproj.user",
    
    // Database files
    "data", "db",
    
    // Web frameworks
    ".svelte-kit", ".routify", ".sapper", 
    ".astro", ".solid", ".qwik",
    
    // Legacy VCS
    ".bzr", ".hg", ".svn", "CVS", "SCCS",
    
    // Tool caches
    ".eslintcache", ".stylelintcache",
    
    // Backup files
    ".backup", "backup", "backups",
    
];

pub const DEFAULT_IGNORE_FILES: &[&str] = &[
    // System files
    ".DS_Store", "Thumbs.db", "desktop.ini",
    
    // Environment and config
    ".env", ".env.local", ".env.development", ".env.production",
    ".envrc", ".direnv",
    
    // Lock files
    "package-lock.json", "yarn.lock", "pnpm-lock.yaml",
    "Cargo.lock", "Pipfile.lock", "poetry.lock",
    "composer.lock", "Gemfile.lock",
    
    // Git files
    ".gitignore", ".gitattributes", ".gitmodules",
    
    // IDE and editor files
    ".vimrc", ".editorconfig", ".clang-format",
    
    // Build and dependency files
    "Makefile", "CMakeLists.txt", "meson.build",
    "requirements.txt", "setup.py", "pyproject.toml",
    "package.json", "tsconfig.json", "webpack.config.js",
    "Dockerfile", "docker-compose.yml", "docker-compose.yaml",
    
    // CI/CD files
    ".travis.yml", ".gitlab-ci.yml", "appveyor.yml",
    "azure-pipelines.yml", "buildspec.yml",
    
    // Temporary and backup files
    "*.tmp", "*.swp", "*.swo", "*.bak", "*.orig", "*~",
    
    // Log files
    "*.log",
    
    // Database files
    "*.db", "*.sqlite", "*.sqlite3",
    
    // Certificate and key files
    "*.pem", "*.key", "*.crt", "*.p12",
    
    // Specific files
    "coder.rs",
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
        .any(|p| 
            ignore_dirs.contains(&p.to_string_lossy().as_ref())
        )
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

/// Returns the absolute path of the input
pub fn abs_file(input: &str) -> String {
    let srcdir = std::path::PathBuf::from(input);
    let c = std::fs::canonicalize(&srcdir).unwrap();
    c.to_string_lossy().to_string()
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
        let path = PathBuf::from("src/node_modules/package");
        assert!(is_ignored_dir(&path));
        
        let path = PathBuf::from("src/main.rs");
        assert!(!is_ignored_dir(&path));
        
        let path = PathBuf::from(".git/config");
        assert!(is_ignored_dir(&path));
    }
    
    #[test]
    fn test_is_ignored_file() {
        assert!(is_ignored_file(".DS_Store"));
        assert!(is_ignored_file("package-lock.json"));
        assert!(is_ignored_file("app.log"));
        assert!(is_ignored_file("data.db"));
        assert!(is_ignored_file("temp.tmp"));
        
        assert!(!is_ignored_file("main.rs"));
        assert!(!is_ignored_file("index.html"));
        assert!(!is_ignored_file("config.json"));
    }
    
    #[test]
    fn test_is_ignored_path() {
        let path = PathBuf::from("src/node_modules/package.json");
        assert!(is_ignored_path(&path));
        
        let path = PathBuf::from("src/.DS_Store");
        assert!(is_ignored_path(&path));
        
        let path = PathBuf::from("src/main.rs");
        assert!(!is_ignored_path(&path));
        
        let path = PathBuf::from("debug.log");
        assert!(is_ignored_path(&path));
    }
}