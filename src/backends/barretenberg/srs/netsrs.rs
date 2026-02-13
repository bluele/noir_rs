use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, RANGE};
use std::ops::Deref;
use std::thread::sleep;
use std::time::Duration;

use super::{Srs, G2};

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

    fn download_g1_data(num_points: u32) -> Vec<u8> {
        let g1_end: u32 = num_points * 64 - 1;

        let mut headers = HeaderMap::new();
        headers.insert(RANGE, format!("bytes={}-{}", 0, g1_end).parse().unwrap());

        Self::download_with_retry("https://crs.aztec.network/g1.dat", Some(headers))
    }

    fn download_g2_data() -> Vec<u8> {
        Self::download_with_retry("https://crs.aztec.network/g2.dat", None)
    }

    fn download_with_retry(url: &str, headers: Option<HeaderMap>) -> Vec<u8> {
        const MAX_RETRIES: usize = 3;
        const CONNECT_TIMEOUT_SECS: u64 = 30;
        const REQUEST_TIMEOUT_SECS: u64 = 120;

        let client = Client::builder()
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .expect("failed to build reqwest client");

        let mut last_error = None;

        for attempt in 1..=MAX_RETRIES {
            let mut request = client.get(url);
            if let Some(headers) = &headers {
                request = request.headers(headers.clone());
            }

            match request.send().and_then(|res| res.error_for_status()) {
                Ok(response) => match response.bytes() {
                    Ok(bytes) => return bytes.to_vec(),
                    Err(err) => last_error = Some(err),
                },
                Err(err) => last_error = Some(err),
            }

            if attempt < MAX_RETRIES {
                sleep(Duration::from_millis((attempt as u64) * 500));
            }
        }

        panic!(
            "failed to download SRS data from {} after {} attempts: {:?}",
            url, MAX_RETRIES, last_error
        );
    }
}
