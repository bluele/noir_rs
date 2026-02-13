use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, RANGE};
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::thread::{sleep, spawn};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

use super::{Srs, G2};

const G1_URL: &str = "https://crs.aztec.network/g1.dat";
const G1_CACHE_PREFIX: &str = "g1_0_";
const G1_CACHE_SUFFIX: &str = ".dat";
const G1_LOCK_FILE: &str = "g1.download.lock";
const MAX_RETRIES: usize = 3;
const CONNECT_TIMEOUT_SECS: u64 = 30;
const REQUEST_TIMEOUT_SECS: u64 = 120;
const LOCK_POLL_INTERVAL_MS: u64 = 200;
const LOCK_STALE_SECS: u64 = 300;

pub struct NetSrs(pub Srs);

impl Deref for NetSrs {
    type Target = Srs;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl NetSrs {
    pub fn new(num_points: u32) -> Result<Self, String> {
        Ok(NetSrs(Srs {
            num_points,
            g1_data: Self::download_g1_data(num_points)?,
            g2_data: G2.to_vec(),
        }))
    }

    pub fn to_srs(self) -> Srs {
        self.0
    }

    fn cache_dir() -> PathBuf {
        let cache_dir = if let Ok(path) = std::env::var("NOIR_RS_SRS_CACHE_DIR") {
            PathBuf::from(path)
        } else if let Ok(path) = std::env::var("XDG_CACHE_HOME") {
            PathBuf::from(path).join("noir_rs").join("cache")
        } else if let Ok(path) = std::env::var("HOME") {
            PathBuf::from(path).join(".noir_rs").join("cache")
        } else {
            PathBuf::from(".noir_rs").join("cache")
        };
        let _ = fs::create_dir_all(&cache_dir);
        cache_dir
    }

    fn cache_file_path(cache_dir: &Path, range_end: u32) -> PathBuf {
        cache_dir.join(format!("{G1_CACHE_PREFIX}{range_end}{G1_CACHE_SUFFIX}"))
    }

    fn parse_cache_range_end(file_name: &str) -> Option<u32> {
        if !file_name.starts_with(G1_CACHE_PREFIX) || !file_name.ends_with(G1_CACHE_SUFFIX) {
            return None;
        }
        let start = G1_CACHE_PREFIX.len();
        let end = file_name.len() - G1_CACHE_SUFFIX.len();
        file_name[start..end].parse::<u32>().ok()
    }

    fn read_cached_file(path: &Path, required_len: usize) -> Option<Vec<u8>> {
        let data = fs::read(path).ok()?;
        if data.len() < required_len {
            debug!(
                "Ignoring SRS cache {} because it is too small ({} < {})",
                path.display(),
                data.len(),
                required_len
            );
            return None;
        }
        Some(data[..required_len].to_vec())
    }

    fn load_cached_g1(cache_dir: &Path, required_len: usize) -> Option<Vec<u8>> {
        let range_end = required_len
            .checked_sub(1)
            .and_then(|v| u32::try_from(v).ok())?;

        let exact_path = Self::cache_file_path(cache_dir, range_end);
        if let Some(data) = Self::read_cached_file(&exact_path, required_len) {
            debug!("Using exact SRS cache {}", exact_path.display());
            return Some(data);
        }

        let mut candidate_paths: Vec<(u32, PathBuf)> = Vec::new();
        let entries = fs::read_dir(cache_dir).ok()?;
        for entry in entries.flatten() {
            let file_name = match entry.file_name().into_string() {
                Ok(name) => name,
                Err(_) => continue,
            };
            let candidate_end = match Self::parse_cache_range_end(&file_name) {
                Some(end) if end >= range_end => end,
                _ => continue,
            };
            candidate_paths.push((candidate_end, entry.path()));
        }
        candidate_paths.sort_by_key(|(end, _)| *end);

        for (candidate_end, path) in candidate_paths {
            if let Some(data) = Self::read_cached_file(&path, required_len) {
                debug!(
                    "Using larger SRS cache {} (range_end={})",
                    path.display(),
                    candidate_end
                );
                return Some(data);
            }
        }

        None
    }

    fn save_cached_g1(cache_dir: &Path, range_end: u32, data: &[u8]) -> Result<(), String> {
        let target = Self::cache_file_path(cache_dir, range_end);
        let temp = target.with_extension(format!("{}.tmp", std::process::id()));

        fs::write(&temp, data)
            .map_err(|err| format!("failed to write temp SRS cache {}: {err}", temp.display()))?;
        fs::rename(&temp, &target).map_err(|err| {
            let _ = fs::remove_file(&temp);
            format!(
                "failed to move temp SRS cache {} to {}: {err}",
                temp.display(),
                target.display()
            )
        })?;
        Ok(())
    }

    fn download_g1_data(num_points: u32) -> Result<Vec<u8>, String> {
        let required_len = (num_points as usize) * 64;
        let range_end = num_points
            .checked_mul(64)
            .and_then(|v| v.checked_sub(1))
            .ok_or_else(|| format!("invalid num_points for SRS download: {num_points}"))?;
        let cache_dir = Self::cache_dir();

        if let Some(cached) = Self::load_cached_g1(&cache_dir, required_len) {
            return Ok(cached);
        }

        let _lock = CacheLockGuard::acquire(&cache_dir)?;

        // Another process may have populated the cache while we were waiting for the lock.
        if let Some(cached) = Self::load_cached_g1(&cache_dir, required_len) {
            return Ok(cached);
        }

        let mut headers = HeaderMap::new();
        headers.insert(
            RANGE,
            format!("bytes={}-{}", 0, range_end)
                .parse()
                .map_err(|err| format!("failed to build SRS range header: {err}"))?,
        );

        let downloaded = Self::download_with_retry(G1_URL, Some(headers))?;
        if downloaded.len() < required_len {
            return Err(format!(
                "downloaded SRS is too short: expected at least {required_len} bytes, got {} bytes",
                downloaded.len()
            ));
        }

        let data = downloaded[..required_len].to_vec();
        if let Err(err) = Self::save_cached_g1(&cache_dir, range_end, &data) {
            warn!("{err}");
        }
        Ok(data)
    }

    fn download_with_retry(url: &str, headers: Option<HeaderMap>) -> Result<Vec<u8>, String> {
        let mut last_error = None;

        for attempt in 1..=MAX_RETRIES {
            match http_get_bytes_on_isolated_thread(
                url,
                headers.clone(),
                Duration::from_secs(CONNECT_TIMEOUT_SECS),
                Duration::from_secs(REQUEST_TIMEOUT_SECS),
            ) {
                Ok(bytes) => return Ok(bytes),
                Err(err) => last_error = Some(err),
            }

            if attempt < MAX_RETRIES {
                debug!(
                    "SRS download attempt {attempt}/{MAX_RETRIES} failed for {url}: {}",
                    last_error.as_deref().unwrap_or("unknown error")
                );
                sleep(Duration::from_millis((attempt as u64) * 500));
            }
        }

        Err(format!(
            "failed to download SRS data from {} after {} attempts: {:?}",
            url, MAX_RETRIES, last_error
        ))
    }
}

struct CacheLockGuard {
    lock_file: PathBuf,
}

impl CacheLockGuard {
    fn acquire(cache_dir: &Path) -> Result<Self, String> {
        let lock_file = cache_dir.join(G1_LOCK_FILE);
        let started_at = Instant::now();
        let timeout = Duration::from_secs(
            REQUEST_TIMEOUT_SECS
                .saturating_mul(MAX_RETRIES as u64)
                .saturating_add(30),
        );

        loop {
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&lock_file)
            {
                Ok(_) => {
                    return Ok(Self { lock_file });
                }
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                    if Self::is_stale(&lock_file).unwrap_or(false) {
                        let _ = fs::remove_file(&lock_file);
                        continue;
                    }
                    if started_at.elapsed() >= timeout {
                        return Err(format!(
                            "timed out waiting for SRS cache lock {}",
                            lock_file.display()
                        ));
                    }
                    sleep(Duration::from_millis(LOCK_POLL_INTERVAL_MS));
                }
                Err(err) => {
                    return Err(format!(
                        "failed to create SRS cache lock {}: {}",
                        lock_file.display(),
                        err
                    ));
                }
            }
        }
    }

    fn is_stale(lock_file: &Path) -> std::io::Result<bool> {
        let metadata = fs::metadata(lock_file)?;
        let modified = metadata.modified()?;
        Ok(modified.elapsed().unwrap_or_default() > Duration::from_secs(LOCK_STALE_SECS))
    }
}

impl Drop for CacheLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_file);
    }
}

fn http_get_bytes_on_isolated_thread(
    url: &str,
    headers: Option<HeaderMap>,
    connect_timeout: Duration,
    request_timeout: Duration,
) -> Result<Vec<u8>, String> {
    let url = url.to_owned();
    let worker = spawn(move || -> Result<Vec<u8>, String> {
        let client = Client::builder()
            .connect_timeout(connect_timeout)
            .timeout(request_timeout)
            .build()
            .map_err(|err| format!("failed to build reqwest client: {err}"))?;

        let mut request = client.get(&url);
        if let Some(headers) = headers {
            request = request.headers(headers);
        }

        let response = request
            .send()
            .and_then(|res| res.error_for_status())
            .map_err(|err| format!("http request failed for {url}: {err}"))?;
        let body = response
            .bytes()
            .map_err(|err| format!("failed to read response body for {url}: {err}"))?;
        Ok(body.to_vec())
    });

    worker
        .join()
        .map_err(|_| "http worker thread panicked".to_string())?
}
