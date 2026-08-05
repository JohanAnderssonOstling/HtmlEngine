use std::io;
use std::path::{Path, PathBuf};

use html_source::{ResourceProvider, TocEntry};

pub struct FileSystemProvider;

impl Default for FileSystemProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSystemProvider {
    pub fn new() -> Self {
        Self
    }

    fn normalize_path(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    fn is_html(path: &Path) -> bool {
        matches!(path.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase()).as_deref(), Some("html") | Some("htm") | Some("xhtml"))
    }

    fn find_html_in_dir(dir: &Path) -> io::Result<Vec<String>> {
        let mut candidates = Vec::new();
        if dir.join("index.html").exists() {
            candidates.push(Self::normalize_path(&dir.join("index.html")));
            return Ok(candidates);
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && Self::is_html(&path) {
                candidates.push(Self::normalize_path(&path));
            }
        }
        Ok(candidates)
    }
}

impl ResourceProvider for FileSystemProvider {
    fn read_bytes(&self, uri: &str) -> io::Result<Vec<u8>> {
        std::fs::read(uri)
    }

    fn exists(&self, uri: &str) -> bool {
        std::fs::metadata(uri).is_ok()
    }

    fn resolve(&self, base: &str, href: &str) -> String {
        let href_path = PathBuf::from(href);
        if href_path.is_absolute() {
            return Self::normalize_path(&href_path);
        }

        let base_path = PathBuf::from(base);
        let base_dir = if base.ends_with(['/', '\\']) || base_path.is_dir() { base_path } else { base_path.parent().map(|p| p.to_path_buf()).unwrap_or(base_path) };
        Self::normalize_path(&base_dir.join(href_path))
    }

    fn list_html_candidates(&self, root: &str) -> io::Result<Vec<String>> {
        let path = PathBuf::from(root);
        if path.is_dir() {
            Self::find_html_in_dir(&path)
        } else if path.is_file() && Self::is_html(&path) {
            Ok(vec![Self::normalize_path(&path)])
        } else {
            Ok(Vec::new())
        }
    }

    fn toc(&self) -> io::Result<Option<Vec<TocEntry>>> {
        Ok(None)
    }
}
