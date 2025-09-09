use super::{Srs, G2};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, RANGE};
use std::fs;
use std::ops::Deref;
use std::path::PathBuf;
use std::time::Duration;

pub struct NetSrs(pub Srs);

impl Deref for NetSrs {
    type Target = Srs;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl NetSrs {
    pub fn new(num_points: u32) -> Self {
        NetSrs(Srs {
            num_points,
            g1_data: Self::download_g1_data(num_points),
            g2_data: G2.to_vec(),
        })
    }

    pub fn to_srs(self) -> Srs {
        self.0
    }

    fn get_cache_dir() -> PathBuf {
        let home_dir = std::env::var("HOME").expect("Could not find home directory");
        let cache_dir = PathBuf::from(home_dir).join(".noir_rs").join("cache");
        fs::create_dir_all(&cache_dir).unwrap_or_else(|_| {});
        cache_dir
    }

    fn get_cache_file_path(range_start: u32, range_end: u32) -> PathBuf {
        let cache_dir = Self::get_cache_dir();
        let filename = format!("g1_{}_{}.dat", range_start, range_end);
        cache_dir.join(filename)
    }

    fn download_g1_data(num_points: u32) -> Vec<u8> {
        let g1_end: u32 = num_points * 64 - 1;
        let range_start = 0;
        let range_end = g1_end;

        // Check if cache file exists
        let cache_file_path = Self::get_cache_file_path(range_start, range_end);

        if cache_file_path.exists() {
            println!("Using cached g1.dat from {:?}", cache_file_path);
            match fs::read(&cache_file_path) {
                Ok(data) => {
                    println!("Successfully loaded cached data");
                    return data;
                }
                Err(e) => {
                    println!("Failed to read cache file: {}. Downloading fresh data.", e);
                }
            }
        }

        // Download if cache doesn't exist or failed to read
        let mut headers = HeaderMap::new();
        headers.insert(
            RANGE,
            format!("bytes={}-{}", range_start, range_end)
                .parse()
                .unwrap(),
        );
        println!("Downloading g1.dat from https://crs.aztec.network/g1.dat");
        println!("Headers: {:?}", headers);

        let data = http_get_bytes_on_isolated_thread(
            "https://crs.aztec.network/g1.dat",
            Some(headers),
            Duration::from_secs(100_000),
        );

        // Save to cache
        match fs::write(&cache_file_path, &data) {
            Ok(_) => {
                println!("Cached downloaded data to {:?}", cache_file_path);
            }
            Err(e) => {
                println!(
                    "Failed to save cache file: {}. Continuing without caching.",
                    e
                );
            }
        }

        data
    }

    fn download_g2_data() -> Vec<u8> {
        http_get_bytes_on_isolated_thread(
            "https://crs.aztec.network/g2.dat",
            None,
            Duration::from_secs(100_000),
        )
    }
}

fn http_get_bytes_on_isolated_thread(
    url: &str,
    headers: Option<HeaderMap>,
    timeout: Duration,
) -> Vec<u8> {
    let url = url.to_string();
    // this is needed to be able to use the client from within an async runtime
    // see https://github.com/seanmonstar/reqwest/pull/1159
    std::thread::spawn(move || {
        let client = Client::new();
        let mut rb = client.get(url).timeout(timeout);
        if let Some(h) = headers {
            rb = rb.headers(h);
        }
        let resp = rb.send().expect("http request failed");
        resp.bytes().expect("failed to read response body").to_vec()
    })
    .join()
    .expect("http worker thread panicked")
}
