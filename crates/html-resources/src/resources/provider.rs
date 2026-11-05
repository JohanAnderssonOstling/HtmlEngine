use std::io;
use std::path::Path;

use html_dom::{ResourceProvider, has_uri_scheme, uri_path};

#[derive(Default)]
pub struct FileSystemProvider;

impl FileSystemProvider {
    pub fn new() -> Self {
        Self
    }

    fn normalize_path(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    fn is_html(path: &Path) -> bool {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                ["html", "htm", "xhtml"]
                    .iter()
                    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
            })
    }

    fn find_html_in_dir(dir: &Path) -> io::Result<Vec<String>> {
        let index = dir.join("index.html");
        if index.exists() {
            return Ok(vec![Self::normalize_path(&index)]);
        }
        let mut candidates = Vec::new();
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
        std::fs::read(uri_path(uri))
    }

    fn exists(&self, uri: &str) -> bool {
        std::fs::metadata(uri_path(uri)).is_ok()
    }

    fn resolve(&self, base: &str, href: &str) -> String {
        if has_uri_scheme(href) || href.starts_with("//") {
            return href.to_owned();
        }
        let href = uri_path(href);
        let base = uri_path(base);
        if href.is_empty() {
            return base.to_owned();
        }
        let href_path = Path::new(href);
        if href_path.is_absolute() {
            return Self::normalize_path(href_path);
        }

        let base_path = Path::new(base);
        let base_dir = if base.ends_with(['/', '\\']) || base_path.is_dir() {
            base_path
        } else {
            base_path.parent().unwrap_or(base_path)
        };
        Self::normalize_path(&base_dir.join(href_path))
    }

    fn list_html_candidates(&self, root: &str) -> io::Result<Vec<String>> {
        let path = Path::new(root);
        if path.is_dir() {
            Self::find_html_in_dir(path)
        } else if path.is_file() && Self::is_html(path) {
            Ok(vec![Self::normalize_path(path)])
        } else {
            Ok(Vec::new())
        }
    }
}

/// Provider for in-memory documents, which have no external resources.
pub struct NoResources;

impl ResourceProvider for NoResources {
    fn read_bytes(&self, _uri: &str) -> io::Result<Vec<u8>> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "in-memory documents have no external resources",
        ))
    }

    fn exists(&self, _uri: &str) -> bool {
        false
    }

    fn resolve(&self, _base: &str, href: &str) -> String {
        href.to_owned()
    }

    fn list_html_candidates(&self, _root: &str) -> io::Result<Vec<String>> {
        Ok(Vec::new())
    }
}
