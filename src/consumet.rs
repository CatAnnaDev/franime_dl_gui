use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ConsumetError {
    #[error("URL Consumet vide")]
    NoUrl,
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Aucun résultat pour « {0} »")]
    NoMatch(String),
    #[error("Pas d'épisode {0} dans les résultats")]
    NoEpisode(usize),
    #[error("Pas de stream pour cet épisode")]
    NoStream,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchResult {
    pub id: String,
    pub title: serde_json::Value,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default, rename = "subOrDub")]
    pub sub_or_dub: Option<String>,
}

impl SearchResult {
    pub fn title_str(&self) -> String {
        match &self.title {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Object(m) => m
                .get("english")
                .or_else(|| m.get("romaji"))
                .or_else(|| m.get("native"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            _ => String::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchResult>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EpisodeInfo {
    pub id: String,
    #[serde(default)]
    pub number: Option<f64>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InfoResponse {
    #[serde(default)]
    episodes: Vec<EpisodeInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamSource {
    pub url: String,
    #[serde(default, rename = "isM3U8")]
    pub is_m3u8: bool,
    #[serde(default)]
    pub quality: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WatchResponse {
    #[serde(default)]
    sources: Vec<StreamSource>,
    #[serde(default)]
    headers: Option<serde_json::Map<String, serde_json::Value>>,
}

pub struct ConsumetClient {
    base_url: String,
    client: reqwest::Client,
}

impl ConsumetClient {
    pub fn new(base_url: &str) -> Result<Self, ConsumetError> {
        if base_url.trim().is_empty() {
            return Err(ConsumetError::NoUrl);
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
        })
    }

    pub async fn search(
        &self,
        provider: &str,
        query: &str,
    ) -> Result<Vec<SearchResult>, ConsumetError> {
        let url = format!(
            "{}/anime/{}/{}",
            self.base_url,
            provider,
            urlencoding(query)
        );
        let resp = self.client.get(&url).send().await?;
        let text = resp.text().await?;
        let parsed: SearchResponse = serde_json::from_str(&text)?;
        Ok(parsed.results)
    }

    pub async fn info(
        &self,
        provider: &str,
        id: &str,
    ) -> Result<Vec<EpisodeInfo>, ConsumetError> {
        let url = format!(
            "{}/anime/{}/info/{}",
            self.base_url,
            provider,
            urlencoding(id)
        );
        let resp = self.client.get(&url).send().await?;
        let text = resp.text().await?;
        let parsed: InfoResponse = serde_json::from_str(&text)?;
        Ok(parsed.episodes)
    }

    pub async fn watch(
        &self,
        provider: &str,
        episode_id: &str,
    ) -> Result<(Vec<StreamSource>, Option<String>), ConsumetError> {
        let url = format!(
            "{}/anime/{}/watch/{}",
            self.base_url,
            provider,
            urlencoding(episode_id)
        );
        let resp = self.client.get(&url).send().await?;
        let text = resp.text().await?;
        let parsed: WatchResponse = serde_json::from_str(&text)?;
        let referer = parsed.headers.as_ref().and_then(|m| {
            m.get("Referer")
                .or_else(|| m.get("referer"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });
        Ok((parsed.sources, referer))
    }

    pub async fn find_episode_url(
        &self,
        provider: &str,
        anime_title: &str,
        alt_titles: &[&str],
        episode_number: usize,
    ) -> Result<(String, bool, Option<String>), ConsumetError> {
        let results = self.search(provider, anime_title).await?;
        if results.is_empty() {
            return Err(ConsumetError::NoMatch(anime_title.to_string()));
        }
        let best = pick_best_match(&results, anime_title, alt_titles)
            .ok_or_else(|| ConsumetError::NoMatch(anime_title.to_string()))?;
        let episodes = self.info(provider, &best.id).await?;
        let ep = episodes
            .iter()
            .find(|e| e.number.map(|n| n as usize) == Some(episode_number))
            .or_else(|| episodes.get(episode_number.saturating_sub(1)))
            .ok_or(ConsumetError::NoEpisode(episode_number))?;
        let (sources, referer) = self.watch(provider, &ep.id).await?;
        let best_source = pick_best_source(&sources).ok_or(ConsumetError::NoStream)?;
        Ok((best_source.url.clone(), best_source.is_m3u8, referer))
    }
}

fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            out.push(c);
        } else {
            for b in c.to_string().bytes() {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut m = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in 0..=a.len() {
        m[i][0] = i;
    }
    for j in 0..=b.len() {
        m[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let c = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            m[i][j] = (m[i - 1][j] + 1).min(m[i][j - 1] + 1).min(m[i - 1][j - 1] + c);
        }
    }
    m[a.len()][b.len()]
}

fn pick_best_match<'a>(
    results: &'a [SearchResult],
    main: &str,
    alts: &[&str],
) -> Option<&'a SearchResult> {
    let needles: Vec<String> = std::iter::once(main.to_string())
        .chain(alts.iter().map(|s| s.to_string()))
        .map(|s| norm(&s))
        .filter(|s| !s.is_empty())
        .collect();
    if needles.is_empty() {
        return None;
    }
    let mut best: Option<(&SearchResult, usize)> = None;
    for r in results {
        let title = norm(&r.title_str());
        if title.is_empty() {
            continue;
        }
        let d = needles
            .iter()
            .map(|n| levenshtein(n, &title))
            .min()
            .unwrap_or(usize::MAX);
        if best.map(|(_, bd)| d < bd).unwrap_or(true) {
            best = Some((r, d));
        }
    }
    let needle_min_len = needles.iter().map(|n| n.len()).min().unwrap_or(0);
    let threshold = (needle_min_len as f32 * 0.5) as usize + 3;
    best.and_then(|(r, d)| if d <= threshold { Some(r) } else { None })
}

fn pick_best_source(sources: &[StreamSource]) -> Option<&StreamSource> {
    let rank = |q: &Option<String>| -> u32 {
        match q.as_deref() {
            Some("1080p") => 100,
            Some("720p") => 80,
            Some("480p") => 60,
            Some("360p") => 40,
            Some("auto") | Some("default") => 90,
            Some("backup") => 30,
            Some(s) => {
                if let Some(n) = s.trim_end_matches('p').parse::<u32>().ok() {
                    n
                } else {
                    50
                }
            }
            None => 50,
        }
    };
    sources.iter().max_by_key(|s| rank(&s.quality))
}
