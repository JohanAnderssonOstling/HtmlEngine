use crate::{MAX_FONT_BYTES, decode};
use fs2::FileExt;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

static CACHE_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

/// Set by the native application before loading fonts, including Android
/// and Kobo where the application owns its private data directory.
pub fn configure_cache_directory(path: PathBuf) {
    let _ = CACHE_DIRECTORY.set(path);
}

#[derive(Clone)]
pub struct FontDiskCache {
    directory: PathBuf,
}

impl FontDiskCache {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub fn configured() -> Result<Self, String> {
        CACHE_DIRECTORY
            .get()
            .cloned()
            .or_else(|| dirs::cache_dir().map(|root| root.join("html-engine/fonts/v1")))
            .map(Self::new)
            .ok_or_else(|| "no persistent font cache directory is available".into())
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn load(&self, url: &str) -> Result<Vec<u8>, String> {
        self.load_with(url, || {
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::limited(10))
                .build()
                .map_err(|error| error.to_string())?;
            let response = client
                .get(url)
                .send()
                .and_then(reqwest::blocking::Response::error_for_status)
                .map_err(|error| error.to_string())?;
            if response
                .content_length()
                .is_some_and(|size| size > MAX_FONT_BYTES as u64)
            {
                return Err("remote font exceeds the size limit".into());
            }
            let mut bytes = Vec::new();
            response
                .take(MAX_FONT_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            Ok(bytes)
        })
    }

    fn load_with(
        &self,
        url: &str,
        fetch: impl FnOnce() -> Result<Vec<u8>, String>,
    ) -> Result<Vec<u8>, String> {
        fs::create_dir_all(&self.directory).map_err(|error| error.to_string())?;
        let key = blake3::hash(url.as_bytes()).to_hex().to_string();
        let path = self.directory.join(format!("{key}.sfnt"));
        // A per-URL OS lock also deduplicates simultaneous chapters, windows,
        // and application processes. It is released automatically on failure.
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.directory.join(format!("{key}.lock")))
            .map_err(|error| error.to_string())?;
        lock.lock_exclusive().map_err(|error| error.to_string())?;
        if fs::metadata(&path).is_ok_and(|metadata| metadata.len() <= MAX_FONT_BYTES as u64)
            && let Ok(bytes) = fs::read(&path)
            && let Ok(decoded) = decode(&bytes)
        {
            return Ok(decoded);
        }
        let bytes = decode(&fetch()?)?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(&self.directory).map_err(|error| error.to_string())?;
        temporary
            .write_all(&bytes)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|error| error.to_string())?;
        temporary
            .persist(&path)
            .map_err(|error| error.to_string())?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn font() -> Vec<u8> {
        include_bytes!("../tests/fixtures/NV_Garamond-Regular.ttf").to_vec()
    }

    #[test]
    fn actual_http_font_is_downloaded_once_and_available_offline() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/font.ttf", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let length = socket.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..length]).starts_with("GET /font.ttf "));
            let bytes = font();
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            socket.write_all(&bytes).unwrap();
        });
        let directory = tempfile::tempdir().unwrap();
        let first = FontDiskCache::new(directory.path()).load(&url).unwrap();
        server.join().unwrap(); // No server is listening for the second request.
        let second = FontDiskCache::new(directory.path()).load(&url).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn disk_cache_survives_new_instances_and_reuses_fonts_offline() {
        let directory = tempfile::tempdir().unwrap();
        let cache = FontDiskCache::new(directory.path());
        let first = cache
            .load_with("https://example.test/font.woff2", || Ok(font()))
            .unwrap();
        let reopened = FontDiskCache::new(directory.path());
        let second = reopened
            .load_with("https://example.test/font.woff2", || {
                panic!("must not download twice")
            })
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn concurrent_requests_download_once() {
        let directory = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let threads = (0..6)
            .map(|_| {
                let calls = calls.clone();
                let cache = FontDiskCache::new(directory.path());
                std::thread::spawn(move || {
                    cache
                        .load_with("https://example.test/font.ttf", || {
                            calls.fetch_add(1, Ordering::SeqCst);
                            Ok(font())
                        })
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failures_are_not_cached_and_corrupt_entries_are_repaired() {
        let directory = tempfile::tempdir().unwrap();
        let cache = FontDiskCache::new(directory.path());
        let url = "https://example.test/font.ttf";
        assert!(cache.load_with(url, || Err("offline".into())).is_err());
        assert!(
            cache
                .load_with(url, || Ok(b"<html>not a font</html>".to_vec()))
                .is_err()
        );
        let expected = cache.load_with(url, || Ok(font())).unwrap();
        let path = directory
            .path()
            .join(format!("{}.sfnt", blake3::hash(url.as_bytes()).to_hex()));
        fs::write(&path, b"interrupted file").unwrap();
        assert_eq!(cache.load_with(url, || Ok(font())).unwrap(), expected);
    }
}
