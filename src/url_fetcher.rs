use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use std::io::Read;

#[derive(Debug, Serialize, Deserialize)]
pub struct VideoResponse {
    pub url: String,
}

pub struct UrlFetcher {
    client: reqwest::blocking::Client,
}

impl UrlFetcher {
    pub fn new() -> Self {
        let client = reqwest::blocking::Client::builder()
            .gzip(true)
            .deflate(true)
            .brotli(true)
            .build()
            .expect("Failed to build HTTP client");

        Self { client }
    }

    pub fn fetch_video_url(
        &self,
        anime_id: u64,
        season: u64,
        episode: u64,
        lang: &str,
        lecteur: u64,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let url = format!(
            "https://api.franime.fr/api/anime/{}/{}/{}/{}/{}",
            anime_id, season, episode, lang, lecteur
        );

        println!("🔗 Fetching URL: {}", url);

        let mut headers = HeaderMap::new();
        headers.insert("Accept", HeaderValue::from_static("*/*"));
        headers.insert("Accept-Encoding", HeaderValue::from_static("zstd"));
        headers.insert(
            "Accept-Language",
            HeaderValue::from_static("en-US,en;q=0.9"),
        );
        headers.insert("Content-Type", HeaderValue::from_static("application/json"));
        headers.insert("Origin", HeaderValue::from_static("https://franime.fr"));
        headers.insert("Priority", HeaderValue::from_static("u=3, i"));
        headers.insert("Referer", HeaderValue::from_static("https://franime.fr/"));
        headers.insert("Sec-Fetch-Dest", HeaderValue::from_static("empty"));
        headers.insert("Sec-Fetch-Mode", HeaderValue::from_static("cors"));
        headers.insert("Sec-Fetch-Site", HeaderValue::from_static("same-site"));
        headers.insert(
            "User-Agent",
            HeaderValue::from_static(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.3 Safari/605.1.15"
            ),
        );

        let response = self.client.get(&url).headers(headers).send()?;

        println!("📊 Status: {}", response.status());

        let bytes = response.bytes()?;

        let decompressed = self.decompress_zstd(&bytes)?;

        let url_str = String::from_utf8(decompressed)?;

        Ok(url_str)
    }

    fn decompress_zstd(&self, data: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut decoder = zstd::Decoder::new(data)?;
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed)?;
        Ok(decompressed)
    }
}

pub struct AsyncUrlFetcher {
    client: reqwest::Client,
}

impl AsyncUrlFetcher {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .gzip(true)
            .deflate(true)
            .brotli(true)
            .build()
            .expect("Failed to build HTTP client");

        Self { client }
    }

    pub async fn fetch_video_url(
        &self,
        anime_id: u64,
        season: u64,
        episode: u64,
        lang: &str,
        lecteur: u64,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let url = format!(
            "https://api.franime.fr/api/anime/{}/{}/{}/{}/{}",
            anime_id, season, episode, lang, lecteur
        );

        println!("🔗 Fetching URL: {}", url);

        let mut headers = HeaderMap::new();
        headers.insert("Accept", HeaderValue::from_static("*/*"));
        headers.insert("Accept-Encoding", HeaderValue::from_static("zstd"));
        headers.insert(
            "Accept-Language",
            HeaderValue::from_static("en-US,en;q=0.9"),
        );
        headers.insert("Content-Type", HeaderValue::from_static("application/json"));
        headers.insert("Origin", HeaderValue::from_static("https://franime.fr"));
        headers.insert("Priority", HeaderValue::from_static("u=3, i"));
        headers.insert("Referer", HeaderValue::from_static("https://franime.fr/"));
        headers.insert("Sec-Fetch-Dest", HeaderValue::from_static("empty"));
        headers.insert("Sec-Fetch-Mode", HeaderValue::from_static("cors"));
        headers.insert("Sec-Fetch-Site", HeaderValue::from_static("same-site"));
        headers.insert(
            "User-Agent",
            HeaderValue::from_static(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.3 Safari/605.1.15"
            ),
        );

        let response = self.client.get(&url).headers(headers).send().await?;

        println!("📊 Status: {}", response.status());

        let bytes = response.bytes().await?;

        let decompressed = Self::decompress_zstd_async(&bytes).await;

        let url_str = String::from_utf8(decompressed)?;

        Ok(url_str)
    }

    async fn decompress_zstd_async(data: &[u8]) -> Vec<u8> {
        let data = data.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut decoder = zstd::Decoder::new(&data[..]).unwrap();
            let mut decompressed = Vec::new();
            decoder.read_to_end(&mut decompressed).unwrap();
            decompressed
        })
        .await
        .unwrap()
    }
}

pub fn parse_lecteur_index(lecteur_string: &str) -> Option<u64> {
    lecteur_string.parse::<u64>().ok().or(Some(0))
}
