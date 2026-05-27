use serde::{Deserialize, Serialize};
use std::time::Duration;

const BASE: &str = "https://anime.uniquestream.net/api/v1";
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36";

#[derive(Debug, thiserror::Error)]
pub enum UsError {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("API: {0}")]
    Api(String),
    #[error("Pas de média HLS")]
    NoHls,
    #[error("Pas de série pour ce content_id (probablement un film/special)")]
    NotASeries,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BrowseSeries {
    pub content_id: String,
    pub title: String,
    #[serde(default)]
    pub subbed: bool,
    #[serde(default)]
    pub dubbed: bool,
    #[serde(default)]
    pub info: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub year: Option<i64>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub episodes_count: Option<i64>,
    #[serde(default)]
    pub seasons_count: Option<i64>,
    #[serde(default)]
    pub score: Option<f32>,
    #[serde(default)]
    pub studio: Option<String>,
    #[serde(default)]
    pub audio_locales: Option<Vec<String>>,
    #[serde(default)]
    pub subtitle_locales: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct BrowseResponse {
    data: Vec<BrowseSeries>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeriesDetail {
    pub content_id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub images: Vec<SeriesImage>,
    #[serde(default)]
    pub seasons: Vec<SeasonInfo>,
    #[serde(default)]
    pub audio_locales: Option<Vec<String>>,
    #[serde(default)]
    pub subtitle_locales: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MovieDetail {
    #[serde(default, rename = "movie_id")]
    pub content_id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub year: Option<i64>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub audio_locales: Option<Vec<String>>,
    #[serde(default)]
    pub subtitle_locales: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub enum ContentDetail {
    Series(SeriesDetail),
    Movie(MovieDetail),
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeriesImage {
    pub url: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeasonInfo {
    pub content_id: String,
    pub title: String,
    #[serde(default)]
    pub season_number: Option<i64>,
    #[serde(default)]
    pub episode_count: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EpisodeInfo {
    pub content_id: String,
    pub title: String,
    #[serde(default)]
    pub episode: Option<String>,
    #[serde(default)]
    pub episode_number: Option<f64>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub audio_locales: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct EpisodeMedia {
    #[serde(default)]
    pub title: Option<String>,
    pub content_id: String,
    #[serde(default)]
    pub hls: Option<HlsBlock>,
}

#[derive(Debug, Deserialize)]
pub struct HlsBlock {
    pub locale: String,
    pub playlist: String,
    #[serde(default)]
    pub hard_subs: Option<Vec<HardSubs>>,
}

#[derive(Debug, Deserialize)]
pub struct HardSubs {
    pub locale: String,
    pub playlist: String,
}

#[derive(Debug, Deserialize)]
pub struct IndexEntry {
    pub prefix: String,
    pub count: i64,
    pub offset: i64,
}

#[derive(Debug, Deserialize)]
pub struct IndexResponse {
    pub total: i64,
    pub data: Vec<IndexEntry>,
}

pub struct UsClient {
    client: reqwest::Client,
}

impl UsClient {
    pub fn new() -> Result<Self, UsError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(25))
            .user_agent(UA)
            .build()?;
        Ok(Self { client })
    }

    pub async fn index_total(&self) -> Result<i64, UsError> {
        let url = format!("{}/videos/index", BASE);
        let text = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?
            .text()
            .await?;
        let parsed: IndexResponse = serde_json::from_str(&text)?;
        Ok(parsed.total)
    }

    pub async fn browse(&self, offset: i64, limit: i64) -> Result<Vec<BrowseSeries>, UsError> {
        let url = format!("{}/videos/browse?offset={}&limit={}", BASE, offset, limit);
        let text = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?
            .text()
            .await?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            UsError::Api(format!(
                "JSON parse: {} (body[:300]={})",
                e,
                &text[..text.len().min(300)]
            ))
        })?;
        let arr = if v.is_array() {
            v.clone()
        } else if let Some(d) = v.get("data") {
            if d.is_array() {
                d.clone()
            } else if let Some(items) = d.get("items") {
                items.clone()
            } else if let Some(data) = d.get("data") {
                data.clone()
            } else {
                return Err(UsError::Api(format!(
                    "shape inconnue (data ni array ni items/data); racine keys = {:?}",
                    v.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>())
                )));
            }
        } else if let Some(items) = v.get("items") {
            items.clone()
        } else if let Some(results) = v.get("results") {
            results.clone()
        } else {
            return Err(UsError::Api(format!(
                "shape inconnue ; racine keys = {:?} body[:300]={}",
                v.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()),
                &text[..text.len().min(300)]
            )));
        };
        let series: Vec<BrowseSeries> = serde_json::from_value(arr).map_err(|e| {
            UsError::Api(format!("deser BrowseSeries: {}", e))
        })?;
        Ok(series)
    }

    pub async fn content(&self, content_id: &str) -> Result<ContentDetail, UsError> {
        match self.series(content_id).await {
            Ok(s) => return Ok(ContentDetail::Series(s)),
            Err(UsError::NotASeries) => {}
            Err(e) => return Err(e),
        }
        let url = format!("{}/content/{}", BASE, content_id);
        let resp = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if status.as_u16() == 404 {
            return Err(UsError::NotASeries);
        }
        if !status.is_success() {
            return Err(UsError::Api(format!(
                "HTTP {} body[:200]={}",
                status,
                &text[..text.len().min(200)]
            )));
        }
        let v: serde_json::Value = serde_json::from_str(&text)?;
        let ct = v.get("content_type").and_then(|x| x.as_str()).unwrap_or("");
        if ct == "movie" {
            let mut movie: MovieDetail = serde_json::from_value(v)?;
            if movie.content_id.is_none() {
                movie.content_id = Some(content_id.to_string());
            }
            Ok(ContentDetail::Movie(movie))
        } else {
            Err(UsError::Api(format!("content_type inconnu: {}", ct)))
        }
    }

    pub async fn movie_media_hls(
        &self,
        movie_content_id: &str,
        locale: &str,
    ) -> Result<String, UsError> {
        let url = format!(
            "{}/movie/{}/media/hls/{}",
            BASE, movie_content_id, locale
        );
        let text = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?
            .text()
            .await?;
        let parsed: EpisodeMedia = serde_json::from_str(&text)?;
        parsed.hls.map(|h| h.playlist).ok_or(UsError::NoHls)
    }

    pub async fn series(&self, content_id: &str) -> Result<SeriesDetail, UsError> {
        let url = format!("{}/series/{}", BASE, content_id);
        let resp = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if status.as_u16() == 404 {
            return Err(UsError::NotASeries);
        }
        if !status.is_success() {
            return Err(UsError::Api(format!(
                "HTTP {} body[:200]={}",
                status,
                &text[..text.len().min(200)]
            )));
        }
        let parsed: SeriesDetail = serde_json::from_str(&text)?;
        Ok(parsed)
    }

    pub async fn episodes(
        &self,
        season_content_id: &str,
        page: i64,
        limit: i64,
    ) -> Result<Vec<EpisodeInfo>, UsError> {
        let url = format!(
            "{}/season/{}/episodes?page={}&limit={}",
            BASE, season_content_id, page, limit
        );
        let text = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?
            .text()
            .await?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            UsError::Api(format!(
                "JSON parse: {} (body[:200]={})",
                e,
                &text[..text.len().min(200)]
            ))
        })?;
        if let Some(arr) = v.as_array() {
            let parsed: Vec<EpisodeInfo> = serde_json::from_value(serde_json::Value::Array(arr.clone()))?;
            return Ok(parsed);
        }
        if let Some(detail) = v.get("detail") {
            return Err(UsError::Api(format!("episodes error: {}", detail)));
        }
        if let Some(items) = v.get("data").or_else(|| v.get("items")) {
            if items.is_array() {
                let parsed: Vec<EpisodeInfo> = serde_json::from_value(items.clone())?;
                return Ok(parsed);
            }
        }
        Err(UsError::Api(format!(
            "episodes shape inconnue: keys = {:?}",
            v.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>())
        )))
    }

    pub async fn episode_media_hls(
        &self,
        episode_content_id: &str,
        locale: &str,
    ) -> Result<String, UsError> {
        let url = format!(
            "{}/episode/{}/media/hls/{}",
            BASE, episode_content_id, locale
        );
        let text = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?
            .text()
            .await?;
        let parsed: EpisodeMedia = serde_json::from_str(&text)?;
        parsed.hls.map(|h| h.playlist).ok_or(UsError::NoHls)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsCachedEpisodes {
    pub audio_locales: Vec<String>,
    pub seasons: Vec<UsCachedSeason>,
    #[serde(default)]
    pub is_movie: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsCachedSeason {
    pub season_content_id: String,
    pub title: String,
    pub season_number: i64,
    pub episodes: Vec<UsCachedEpisode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsCachedEpisode {
    pub content_id: String,
    pub title: String,
    pub episode_number: f64,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub image: Option<String>,
}

pub async fn fetch_all_episodes(
    client: &UsClient,
    content_id: &str,
) -> Result<UsCachedEpisodes, UsError> {
    let content = client.content(content_id).await?;
    let detail = match content {
        ContentDetail::Series(s) => s,
        ContentDetail::Movie(m) => {
            let title = if m.title.is_empty() {
                "Film".to_string()
            } else {
                m.title.clone()
            };
            let mut out = UsCachedEpisodes {
                audio_locales: m.audio_locales.clone().unwrap_or_default(),
                seasons: vec![UsCachedSeason {
                    season_content_id: content_id.to_string(),
                    title: "Film".to_string(),
                    season_number: 1,
                    episodes: vec![UsCachedEpisode {
                        content_id: content_id.to_string(),
                        title,
                        episode_number: 1.0,
                        duration_ms: m.duration_ms,
                        image: m.image.clone(),
                    }],
                }],
                is_movie: true,
            };
            if out.audio_locales.is_empty() {
                out.audio_locales = vec!["ja-JP".to_string()];
            }
            return Ok(out);
        }
    };
    let mut out = UsCachedEpisodes {
        audio_locales: detail.audio_locales.clone().unwrap_or_default(),
        seasons: Vec::new(),
        is_movie: false,
    };
    for s in &detail.seasons {
        let mut eps: Vec<UsCachedEpisode> = Vec::new();
        let mut page = 1i64;
        let per_page = 20i64;
        loop {
            let batch = client.episodes(&s.content_id, page, per_page).await?;
            if batch.is_empty() {
                break;
            }
            let n = batch.len();
            for e in batch {
                eps.push(UsCachedEpisode {
                    content_id: e.content_id,
                    title: e.title,
                    episode_number: e.episode_number.unwrap_or(eps.len() as f64 + 1.0),
                    duration_ms: e.duration_ms,
                    image: e.image,
                });
            }
            if (n as i64) < per_page {
                break;
            }
            page += 1;
        }
        out.seasons.push(UsCachedSeason {
            season_content_id: s.content_id.clone(),
            title: s.title.clone(),
            season_number: s.season_number.unwrap_or(0),
            episodes: eps,
        });
    }
    Ok(out)
}

pub fn pick_audio_locale(audio_locales: &Option<Vec<String>>, prefer_dub: bool) -> String {
    let locales: Vec<&str> = audio_locales
        .as_ref()
        .map(|v| v.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();
    if prefer_dub {
        for candidate in &["fr-FR", "en-US", "es-ES", "es-419", "de-DE", "pt-BR", "it-IT"] {
            if locales.iter().any(|l| l == candidate) {
                return candidate.to_string();
            }
        }
    }
    if locales.iter().any(|l| *l == "ja-JP") {
        return "ja-JP".to_string();
    }
    locales
        .first()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "ja-JP".to_string())
}
