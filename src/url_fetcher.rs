use reqwest::header::{HeaderMap, HeaderValue, CONTENT_ENCODING};
use std::io::Read;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Server returned status {0}")]
    Status(reqwest::StatusCode),
    #[error("Decompression error: {0}")]
    Decompress(#[from] std::io::Error),
    #[error("Invalid UTF-8 in response: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("Task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
}

#[derive(Clone)]
pub struct UrlFetcher {
    client: reqwest::Client,
}

impl UrlFetcher {
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
    ) -> Result<String, FetchError> {
        let url = format!(
            "https://api.franime.fr/api/anime/{}/{}/{}/{}/{}",
            anime_id, season, episode, lang, lecteur
        );

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
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
            ),
        );

        let response = self.client.get(&url).headers(headers).send().await?;

        if !response.status().is_success() {
            return Err(FetchError::Status(response.status()));
        }

        let is_zstd = response
            .headers()
            .get(CONTENT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.eq_ignore_ascii_case("zstd"))
            .unwrap_or(false);

        let bytes = response.bytes().await?;

        let body = if is_zstd {
            decompress_zstd(bytes.to_vec()).await?
        } else {
            bytes.to_vec()
        };

        Ok(String::from_utf8(body)?)
    }
}

impl Default for UrlFetcher {
    fn default() -> Self {
        Self::new()
    }
}

async fn decompress_zstd(data: Vec<u8>) -> Result<Vec<u8>, FetchError> {
    let result = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        let mut decoder = zstd::Decoder::new(&data[..])?;
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed)?;
        Ok(decompressed)
    })
    .await??;
    Ok(result)
}
