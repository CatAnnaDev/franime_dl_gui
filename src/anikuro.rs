use serde::Deserialize;
use std::time::Duration;

const ANIKURO_BASE: &str = "https://anikuro.to";

#[derive(Debug, thiserror::Error)]
pub enum AnikuroError {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("API: {0}")]
    Api(String),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Aucun résultat pour « {0} »")]
    NoMatch(String),
    #[error("Pas de source streaming")]
    NoStream,
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    ok: bool,
    data: Option<T>,
    #[serde(default)]
    error: Option<ApiError>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct SearchData {
    #[serde(default)]
    items: Vec<SearchItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchItem {
    #[serde(default, rename = "anilistId")]
    pub anilist_id: Option<i64>,
    #[serde(default)]
    pub title: Option<TitleObj>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TitleObj {
    #[serde(default)]
    pub romaji: Option<String>,
    #[serde(default)]
    pub english: Option<String>,
    #[serde(default)]
    pub native: Option<String>,
    #[serde(default, rename = "userPreferred")]
    pub user_preferred: Option<String>,
}

impl TitleObj {
    fn all_titles(&self) -> Vec<String> {
        let mut out = Vec::new();
        for t in [
            &self.english,
            &self.romaji,
            &self.user_preferred,
            &self.native,
        ] {
            if let Some(s) = t {
                if !s.is_empty() {
                    out.push(s.clone());
                }
            }
        }
        out
    }
}

#[derive(Debug, Deserialize)]
struct SourcesData {
    #[serde(default)]
    raw: Option<SourcesRaw>,
}

#[derive(Debug, Deserialize)]
struct SourcesRaw {
    #[serde(default)]
    sub: Option<SubObj>,
    #[serde(default)]
    dub: Option<SubObj>,
}

#[derive(Debug, Deserialize)]
struct SubObj {
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    headers: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    sources: Option<Vec<SourceEntry>>,
}

#[derive(Debug, Deserialize)]
struct SourceEntry {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    quality: Option<String>,
}

pub struct AnikuroClient {
    client: reqwest::Client,
}

impl AnikuroClient {
    pub fn new() -> Result<Self, AnikuroError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(25))
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36")
            .build()?;
        Ok(Self { client })
    }

    pub async fn search(&self, query: &str) -> Result<Vec<SearchItem>, AnikuroError> {
        let url = format!(
            "{}/api/v1/discovery/search?q={}",
            ANIKURO_BASE,
            url_encode(query)
        );
        let text = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?
            .text()
            .await?;
        let env: Envelope<SearchData> = serde_json::from_str(&text)?;
        if !env.ok {
            let msg = env
                .error
                .map(|e| e.message)
                .unwrap_or_else(|| "search failed".into());
            return Err(AnikuroError::Api(msg));
        }
        Ok(env.data.map(|d| d.items).unwrap_or_default())
    }

    pub async fn sources(
        &self,
        provider: &str,
        anilist_id: i64,
        episode: usize,
        prefer_dub: bool,
    ) -> Result<(String, Option<String>), AnikuroError> {
        let path = format!(
            "{}/api/v1/sources/{}/{}:{}",
            ANIKURO_BASE, provider, anilist_id, episode
        );
        let text = self
            .client
            .get(&path)
            .header("Accept", "application/json")
            .send()
            .await?
            .text()
            .await?;
        let env: Envelope<SourcesData> = serde_json::from_str(&text)?;
        if !env.ok {
            let msg = env
                .error
                .map(|e| e.message)
                .unwrap_or_else(|| "sources failed".into());
            return Err(AnikuroError::Api(msg));
        }
        let raw = env.data.and_then(|d| d.raw).ok_or(AnikuroError::NoStream)?;
        let track = if prefer_dub {
            raw.dub.or(raw.sub)
        } else {
            raw.sub.or(raw.dub)
        }
        .ok_or(AnikuroError::NoStream)?;

        let referer = track.headers.as_ref().and_then(|m| {
            m.get("Referer")
                .or_else(|| m.get("referer"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });

        if let Some(sources) = track.sources {
            if let Some(best) = pick_best_quality(&sources) {
                if let Some(url) = best.url.clone() {
                    return Ok((url, referer));
                }
            }
        }
        if let Some(url) = track.default {
            return Ok((url, referer));
        }
        Err(AnikuroError::NoStream)
    }

    pub async fn find_stream(
        &self,
        provider: &str,
        anime_title: &str,
        alt_titles: &[&str],
        episode_number: usize,
        prefer_dub: bool,
    ) -> Result<(String, Option<String>), AnikuroError> {
        let items = self.search(anime_title).await?;
        if items.is_empty() {
            return Err(AnikuroError::NoMatch(anime_title.to_string()));
        }
        let best = pick_best_match(&items, anime_title, alt_titles)
            .ok_or_else(|| AnikuroError::NoMatch(anime_title.to_string()))?;
        let anilist_id = best.anilist_id.ok_or(AnikuroError::NoStream)?;
        self.sources(provider, anilist_id, episode_number, prefer_dub).await
    }
}

fn url_encode(s: &str) -> String {
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
    items: &'a [SearchItem],
    main: &str,
    alts: &[&str],
) -> Option<&'a SearchItem> {
    let needles: Vec<String> = std::iter::once(main.to_string())
        .chain(alts.iter().map(|s| s.to_string()))
        .map(|s| norm(&s))
        .filter(|s| !s.is_empty())
        .collect();
    if needles.is_empty() {
        return None;
    }
    let mut best: Option<(&SearchItem, usize)> = None;
    for it in items {
        let titles = it.title.as_ref().map(|t| t.all_titles()).unwrap_or_default();
        for t in &titles {
            let nt = norm(t);
            if nt.is_empty() {
                continue;
            }
            let d = needles.iter().map(|n| levenshtein(n, &nt)).min().unwrap_or(usize::MAX);
            if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((it, d));
            }
        }
    }
    let min_needle = needles.iter().map(|n| n.len()).min().unwrap_or(0);
    let threshold = (min_needle as f32 * 0.5) as usize + 3;
    best.and_then(|(it, d)| if d <= threshold { Some(it) } else { None })
}

fn pick_best_quality(sources: &[SourceEntry]) -> Option<&SourceEntry> {
    let rank = |q: &Option<String>| -> u32 {
        match q.as_deref() {
            Some("1080p") => 100,
            Some("720p") => 80,
            Some("480p") => 60,
            Some("360p") => 40,
            Some("auto") | Some("default") => 90,
            Some(s) => s.trim_end_matches('p').parse::<u32>().unwrap_or(50),
            None => 50,
        }
    };
    sources.iter().max_by_key(|s| rank(&s.quality))
}
