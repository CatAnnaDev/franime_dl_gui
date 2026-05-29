use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const BASE: &str = "https://anime-sama.to";
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36";

#[derive(Debug, thiserror::Error)]
pub enum AsError {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("HTTP status {0}")]
    Status(u16),
    #[error("Aucune donnée")]
    NoData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsSeries {
    pub slug: String,
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub image: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsSource {
    pub host: String,
    pub iframe: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsEpisode {
    pub number: f64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub vo: Vec<AsSource>,
    #[serde(default)]
    pub vf: Vec<AsSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsSeason {
    pub title: String,
    pub episodes: Vec<AsEpisode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AsCachedEpisodes {
    pub seasons: Vec<AsSeason>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub title: String,
}

struct Section {
    base: String,
    lang: String,
}

pub struct AsClient {
    client: reqwest::Client,
}

impl AsClient {
    pub fn new() -> Result<Self, AsError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(UA)
            .build()?;
        Ok(Self { client })
    }

    async fn get_text(&self, url: &str) -> Result<String, AsError> {
        let resp = self
            .client
            .get(url)
            .header("Accept", "text/html,application/xhtml+xml,*/*")
            .header("Accept-Language", "fr-FR,fr;q=0.9,en;q=0.8")
            .header("Referer", BASE)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(AsError::Status(status.as_u16()));
        }
        let bytes = resp.bytes().await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub async fn fetch_sitemap(&self) -> Result<Vec<(String, String)>, AsError> {
        let xml = self.get_text(&format!("{}/sitemap.xml", BASE)).await?;
        let re = regex::Regex::new(
            r"(?is)<loc>\s*(.*?)\s*</loc>\s*<lastmod>\s*(.*?)\s*</lastmod>",
        )
        .map_err(|_| AsError::NoData)?;
        let mut out = Vec::new();
        for c in re.captures_iter(&xml) {
            let loc = c.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
            let lastmod = c.get(2).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
            if let Some(slug) = slug_from_catalogue_url(&loc) {
                out.push((slug, lastmod));
            }
        }
        Ok(out)
    }

    pub async fn anime_detail(&self, slug: &str) -> Result<AsCachedEpisodes, AsError> {
        let url = format!("{}/catalogue/{}/", BASE, slug);
        let html = self.get_text(&url).await?;

        let title = meta_content(&html, "og:title")
            .or_else(|| extract_title_tag(&html))
            .map(|t| clean_title(&t))
            .unwrap_or_else(|| slug.to_string());
        let description = meta_content(&html, "og:description").unwrap_or_default();
        let image = meta_content(&html, "og:image");
        let title_lower = title.to_lowercase();
        let genres: Vec<String> = parse_genres(&html)
            .into_iter()
            .filter(|g| g.to_lowercase() != title_lower)
            .collect();

        let mut sections = parse_sections(&html);
        sections.sort_by(|a, b| a.base.cmp(&b.base));
        sections.dedup_by(|a, b| a.base == b.base && a.lang == b.lang);

        let mut bases: Vec<String> = Vec::new();
        for s in &sections {
            if !bases.contains(&s.base) {
                bases.push(s.base.clone());
            }
        }

        if bases.is_empty() {
            return Ok(AsCachedEpisodes {
                seasons: Vec::new(),
                description,
                genres,
                image,
                title,
            });
        }

        let mut seasons: Vec<AsSeason> = Vec::new();
        for base in &bases {
            let has_vo = sections.iter().any(|s| &s.base == base && s.lang == "vostfr");
            let has_vf = sections.iter().any(|s| &s.base == base && s.lang == "vf");
            let vo_lists = if has_vo {
                self.fetch_episodes_js(slug, base, "vostfr").await.unwrap_or_default()
            } else {
                Vec::new()
            };
            let vf_lists = if has_vf {
                self.fetch_episodes_js(slug, base, "vf").await.unwrap_or_default()
            } else {
                Vec::new()
            };
            let vo_eps = transpose_players(&vo_lists);
            let vf_eps = transpose_players(&vf_lists);
            let count = vo_eps.len().max(vf_eps.len());
            if count == 0 {
                continue;
            }
            let mut episodes = Vec::with_capacity(count);
            for i in 0..count {
                episodes.push(AsEpisode {
                    number: (i + 1) as f64,
                    title: format!("Épisode {}", i + 1),
                    vo: vo_eps.get(i).cloned().unwrap_or_default(),
                    vf: vf_eps.get(i).cloned().unwrap_or_default(),
                });
            }
            seasons.push(AsSeason {
                title: pretty_section_name(base),
                episodes,
            });
        }

        if seasons.is_empty() {
            return Err(AsError::NoData);
        }

        Ok(AsCachedEpisodes {
            seasons,
            description,
            genres,
            image,
            title,
        })
    }

    async fn fetch_episodes_js(
        &self,
        slug: &str,
        base: &str,
        lang: &str,
    ) -> Result<Vec<Vec<AsSource>>, AsError> {
        let url = format!("{}/catalogue/{}/{}/{}/episodes.js", BASE, slug, base, lang);
        let js = self.get_text(&url).await?;
        Ok(parse_episodes_js(&js))
    }
}

fn slug_from_catalogue_url(url: &str) -> Option<String> {
    let marker = "/catalogue/";
    let idx = url.find(marker)?;
    let rest = url[idx + marker.len()..].trim_end_matches('/');
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    Some(rest.to_string())
}

fn extract_title_tag(html: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?is)<title>(.*?)</title>").ok()?;
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

fn clean_title(raw: &str) -> String {
    let t = raw.split('|').next().unwrap_or(raw);
    t.trim().to_string()
}

fn meta_content(html: &str, prop: &str) -> Option<String> {
    let pat = format!(
        r#"(?is)<meta[^>]+(?:property|name)=["']{}["'][^>]+content=["']([^"']*)["']"#,
        regex::escape(prop)
    );
    let re = regex::Regex::new(&pat).ok()?;
    let v = re
        .captures(html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())?;
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

fn parse_genres(html: &str) -> Vec<String> {
    let re = match regex::Regex::new(r#"(?is)<meta[^>]+name=["']keywords["'][^>]+content=["']([^"']*)["']"#) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    const JUNK: &[&str] = &[
        "anime-sama",
        "anime sama",
        "anime",
        "animes",
        "scan",
        "scans",
        "vostfr",
        "vf",
        "va",
        "vo",
        "streaming",
        "manga",
        "mangas",
        "catalogue",
    ];
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| {
            m.as_str()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .filter(|s| !JUNK.contains(&s.to_lowercase().as_str()))
                .take(8)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_sections(html: &str) -> Vec<Section> {
    let re = match regex::Regex::new(r#"(?is)panneauAnime\(\s*"[^"]*"\s*,\s*"([^"]+)"\s*\)"#) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for c in re.captures_iter(html) {
        let Some(path) = c.get(1).map(|m| m.as_str().trim().to_string()) else {
            continue;
        };
        if path.contains("nom") || path.is_empty() {
            continue;
        }
        let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
        if parts.len() < 2 {
            continue;
        }
        let lang = parts[parts.len() - 1].to_lowercase();
        if lang != "vostfr" && lang != "vf" && lang != "va" {
            continue;
        }
        let base = parts[..parts.len() - 1].join("/");
        let lang = if lang == "vf" || lang == "va" {
            "vf".to_string()
        } else {
            "vostfr".to_string()
        };
        out.push(Section { base, lang });
    }
    out
}

fn pretty_section_name(base: &str) -> String {
    let b = base.trim_matches('/');
    if let Some(n) = b.strip_prefix("saison") {
        if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
            return format!("Saison {}", n);
        }
    }
    match b {
        "film" | "films" => "Films".to_string(),
        "oav" => "OAV".to_string(),
        other => {
            let mut s = other.replace(['-', '_'], " ");
            if let Some(first) = s.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            s
        }
    }
}

fn parse_episodes_js(js: &str) -> Vec<Vec<AsSource>> {
    let mut players: Vec<Vec<AsSource>> = Vec::new();
    let bytes = js.as_bytes();
    let decl = match regex::Regex::new(r"(?i)var\s+eps\w+\s*=\s*\[") {
        Ok(r) => r,
        Err(_) => return players,
    };
    for m in decl.find_iter(js) {
        let open = m.end() - 1;
        let Some(close) = match_bracket(bytes, open) else {
            continue;
        };
        let body = &js[open + 1..close];
        let urls = extract_quoted_urls(body);
        if !urls.is_empty() {
            players.push(
                urls.into_iter()
                    .map(|u| AsSource {
                        host: host_of(&u),
                        iframe: u,
                    })
                    .collect(),
            );
        }
    }
    players
}

fn match_bracket(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_str: Option<u8> = None;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        if escape {
            escape = false;
            continue;
        }
        if b == b'\\' {
            escape = true;
            continue;
        }
        if let Some(q) = in_str {
            if b == q {
                in_str = None;
            }
            continue;
        }
        match b {
            b'\'' | b'"' => in_str = Some(b),
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn extract_quoted_urls(body: &str) -> Vec<String> {
    let re = match regex::Regex::new(r#"["'](https?://[^"']+)["']"#) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    re.captures_iter(body)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

fn transpose_players(players: &[Vec<AsSource>]) -> Vec<Vec<AsSource>> {
    let count = players.iter().map(|p| p.len()).max().unwrap_or(0);
    let mut out: Vec<Vec<AsSource>> = vec![Vec::new(); count];
    for p in players {
        for (i, src) in p.iter().enumerate() {
            if !src.iframe.is_empty() {
                out[i].push(src.clone());
            }
        }
    }
    out
}

fn host_of(url: &str) -> String {
    let trimmed = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = trimmed.split('/').next().unwrap_or(trimmed);
    let host = host.trim_start_matches("www.");
    host.split('.').next().unwrap_or(host).to_string()
}

pub fn as_id_from(slug: &str) -> f64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in slug.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let masked = hash & ((1u64 << 52) - 1);
    let f = masked as f64;
    -(f + 2.0)
}
