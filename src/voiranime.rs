use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const BASE: &str = "https://voir-anime.to";
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36";

#[derive(Debug, thiserror::Error)]
pub enum VaError {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Parse: {0}")]
    Parse(String),
    #[error("Pas d'iframe trouvée")]
    NoIframe,
    #[error("HTTP status {0}")]
    Status(u16),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaSeries {
    pub slug: String,
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub image: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VaDetail {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub year: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub score: Option<f32>,
    #[serde(default)]
    pub episodes: Vec<VaEpisode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaEpisode {
    pub number: f64,
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub lang: String,
    #[serde(default)]
    pub sources: Vec<VaSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaSource {
    pub host: String,
    pub iframe: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VaCachedEpisodes {
    pub episodes: Vec<VaEpisode>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub year: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub score: Option<f32>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub title: String,
}

pub struct VaClient {
    client: reqwest::Client,
}

impl VaClient {
    pub fn new() -> Result<Self, VaError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(UA)
            .build()?;
        Ok(Self { client })
    }

    async fn get_html(&self, url: &str) -> Result<String, VaError> {
        let resp = self
            .client
            .get(url)
            .header("Accept", "text/html,application/xhtml+xml")
            .header("Accept-Language", "fr-FR,fr;q=0.9,en;q=0.8")
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(VaError::Status(status.as_u16()));
        }
        let bytes = resp.bytes().await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub async fn browse_page(&self, page: i64) -> Result<Vec<VaSeries>, VaError> {
        let url = if page <= 1 {
            format!("{}/liste-danimes/", BASE)
        } else {
            format!("{}/liste-danimes/page/{}/", BASE, page)
        };
        let html = self.get_html(&url).await?;
        Ok(parse_list(&html))
    }

    pub async fn anime_detail(&self, slug: &str) -> Result<VaDetail, VaError> {
        let url = format!("{}/anime/{}/", BASE, slug);
        let html = self.get_html(&url).await?;
        parse_detail(slug, &html)
    }

    pub async fn episode_iframe(&self, episode_url: &str) -> Result<String, VaError> {
        let html = self.get_html(episode_url).await?;
        parse_iframe(&html).ok_or(VaError::NoIframe)
    }

    pub async fn episode_sources(&self, episode_url: &str) -> Result<Vec<VaSource>, VaError> {
        let html = self.get_html(episode_url).await?;
        let mut sources = parse_chapter_sources(&html);
        if sources.is_empty() {
            if let Some(iframe) = parse_iframe(&html) {
                sources.push(VaSource {
                    host: "default".to_string(),
                    iframe,
                });
            } else {
                return Err(VaError::NoIframe);
            }
        }
        Ok(sources)
    }
}

fn parse_list(html: &str) -> Vec<VaSeries> {
    let mut out: Vec<VaSeries> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let id_re = regex::Regex::new(r#"(?is)<div\s+id="manga-item-(\d+)""#).unwrap();
    let href_re = regex::Regex::new(
        r#"(?is)href="(https://voir-anime\.to/anime/([^"/]+)/)""#,
    )
    .unwrap();
    let title_attr_re = regex::Regex::new(r#"(?is)title="([^"]+)""#).unwrap();
    let post_title_re = regex::Regex::new(
        r#"(?is)<div class="post-title[^"]*">.*?<a[^>]+href="https://voir-anime\.to/anime/[^"]+/"[^>]*>([^<]+)</a>"#,
    )
    .unwrap();
    let img_re = regex::Regex::new(
        r#"(?is)<img[^>]+(?:data-src|src)="([^"]+)""#,
    )
    .unwrap();

    let positions: Vec<usize> = id_re.find_iter(html).map(|m| m.start()).collect();
    if positions.is_empty() {
        return out;
    }

    for i in 0..positions.len() {
        let start = positions[i];
        let end = if i + 1 < positions.len() { positions[i + 1] } else { html.len() };
        let block = &html[start..end];

        let Some(href_cap) = href_re.captures(block) else { continue };
        let url = href_cap.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
        let slug = href_cap.get(2).map(|m| m.as_str().to_string()).unwrap_or_default();
        if slug.is_empty() || seen.contains(&slug) {
            continue;
        }
        let title_from_post = post_title_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| decode_entities(m.as_str()).trim().to_string());
        let title_from_attr = title_attr_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| decode_entities(m.as_str()).trim().to_string());
        let title = title_from_post
            .or(title_from_attr)
            .unwrap_or_else(|| slug.clone());
        let image = img_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string());

        seen.insert(slug.clone());
        out.push(VaSeries {
            slug,
            title,
            url,
            image,
        });
    }

    out
}

fn parse_detail(slug: &str, html: &str) -> Result<VaDetail, VaError> {
    let title_re = regex::Regex::new(
        r#"(?is)<div class="post-title[^"]*">\s*<h1[^>]*>(.*?)</h1>"#,
    )
    .unwrap();
    let summary_re = regex::Regex::new(
        r#"(?is)<div class="summary__content[^"]*">(.*?)</div>"#,
    )
    .unwrap();
    let poster_re = regex::Regex::new(
        r#"(?is)<div class="summary_image">.*?<img[^>]+(?:data-src|src)="([^"]+)""#,
    )
    .unwrap();
    let genres_re = regex::Regex::new(
        r#"(?is)<div class="genres-content">(.*?)</div>"#,
    )
    .unwrap();
    let genre_link_re = regex::Regex::new(r#"(?is)<a[^>]*>([^<]+)</a>"#).unwrap();
    let score_re = regex::Regex::new(
        r#"(?is)<span id="averagerate"[^>]*>\s*([0-9]+(?:\.[0-9]+)?)\s*</span>"#,
    )
    .unwrap();
    let release_re = regex::Regex::new(
        r#"(?is)<div class="post-content_item">\s*<div class="summary-heading">\s*<h5>\s*(?:Release|Sortie)[^<]*</h5>.*?<div class="summary-content[^"]*">\s*<a[^>]*>([^<]+)</a>"#,
    )
    .unwrap();
    let status_re = regex::Regex::new(
        r#"(?is)<div class="post-content_item">\s*<div class="summary-heading">\s*<h5>\s*Status[^<]*</h5>\s*</div>\s*<div class="summary-content[^"]*">([^<]+)</div>"#,
    )
    .unwrap();

    let title = title_re
        .captures(html)
        .and_then(|c| c.get(1))
        .map(|m| decode_entities(&strip_tags(m.as_str())).trim().to_string())
        .unwrap_or_else(|| slug.to_string());
    let description = summary_re
        .captures(html)
        .and_then(|c| c.get(1))
        .map(|m| decode_entities(&strip_tags(m.as_str())).trim().to_string())
        .unwrap_or_default();
    let image = poster_re
        .captures(html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string());
    let mut genres: Vec<String> = Vec::new();
    if let Some(c) = genres_re.captures(html) {
        if let Some(block) = c.get(1) {
            for g in genre_link_re.captures_iter(block.as_str()) {
                if let Some(n) = g.get(1) {
                    let t = decode_entities(n.as_str()).trim().to_string();
                    if !t.is_empty() {
                        genres.push(t);
                    }
                }
            }
        }
    }
    let score = score_re
        .captures(html)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<f32>().ok());
    let year = release_re
        .captures(html)
        .and_then(|c| c.get(1))
        .map(|m| decode_entities(m.as_str()).trim().to_string());
    let status = status_re
        .captures(html)
        .and_then(|c| c.get(1))
        .map(|m| decode_entities(m.as_str()).trim().to_string());

    let episodes = parse_episodes(slug, html);

    Ok(VaDetail {
        slug: slug.to_string(),
        title,
        description,
        image,
        genres,
        year,
        status,
        score,
        episodes,
    })
}

fn parse_episodes(slug: &str, html: &str) -> Vec<VaEpisode> {
    let ep_url_re_str = format!(
        r#"(?is)<a[^>]+href="(https://voir-anime\.to/anime/{}/([^"]+)/)""#,
        regex::escape(slug)
    );
    let ep_url_re = match regex::Regex::new(&ep_url_re_str) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut map: std::collections::BTreeMap<u64, VaEpisode> = std::collections::BTreeMap::new();
    let mut any: Vec<VaEpisode> = Vec::new();
    let num_re = regex::Regex::new(r"(?i)(?:^|[^0-9])(\d+)(?:-vostfr|-vf|-vostf|$)").unwrap();
    for cap in ep_url_re.captures_iter(html) {
        let url = cap.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
        let ep_slug = cap.get(2).map(|m| m.as_str().to_string()).unwrap_or_default();
        if ep_slug.is_empty() {
            continue;
        }
        let lang = if ep_slug.contains("-vf") {
            "vf".to_string()
        } else {
            "vostfr".to_string()
        };
        let number_int: Option<u64> = num_re
            .captures(&ep_slug)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse::<u64>().ok());
        let number = number_int.unwrap_or(0) as f64;
        let title = format!("Épisode {}", number_int.unwrap_or(0));
        let ep = VaEpisode {
            number,
            url: url.clone(),
            title,
            lang,
            sources: Vec::new(),
        };
        if let Some(n) = number_int {
            map.entry(n).or_insert(ep.clone());
        }
        any.push(ep);
    }
    if !map.is_empty() {
        map.into_values().collect()
    } else {
        let mut seen_urls: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut dedup: Vec<VaEpisode> = Vec::new();
        for e in any {
            if seen_urls.insert(e.url.clone()) {
                dedup.push(e);
            }
        }
        dedup
    }
}

fn parse_chapter_sources(html: &str) -> Vec<VaSource> {
    let mut out = Vec::new();
    let Some(start) = html.find("thisChapterSources") else {
        return out;
    };
    let after_eq = &html[start..];
    let Some(brace_off) = after_eq.find('{') else {
        return out;
    };
    let body_start = start + brace_off;
    let bytes = html.as_bytes();
    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut escape = false;
    let mut end_idx: Option<usize> = None;
    for i in body_start..bytes.len() {
        let b = bytes[i];
        if escape {
            escape = false;
            continue;
        }
        if b == b'\\' {
            escape = true;
            continue;
        }
        if b == b'"' {
            in_str = !in_str;
            continue;
        }
        if in_str {
            continue;
        }
        if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                end_idx = Some(i + 1);
                break;
            }
        }
    }
    let Some(end) = end_idx else {
        return out;
    };
    let json_slice = &html[body_start..end];

    let parsed: Result<serde_json::Map<String, serde_json::Value>, _> =
        serde_json::from_str(json_slice);
    let Ok(map) = parsed else {
        return out;
    };
    let src_re = match regex::Regex::new(r#"(?is)<iframe[^>]+src=["']([^"']+)["']"#) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for (host, value) in map {
        let html_str = match value.as_str() {
            Some(s) => s.to_string(),
            None => continue,
        };
        let Some(cap) = src_re.captures(&html_str) else {
            continue;
        };
        let Some(src) = cap.get(1) else { continue };
        let iframe = src.as_str().to_string();
        if is_blacklisted_iframe(&iframe) {
            continue;
        }
        let host_clean = host
            .strip_prefix("LECTEUR ")
            .unwrap_or(&host)
            .trim()
            .to_string();
        let host_final = if host_clean.is_empty() {
            host
        } else {
            host_clean
        };
        out.push(VaSource {
            host: host_final,
            iframe,
        });
    }
    out
}

const KNOWN_VIDEO_HOSTS: &[&str] = &[
    "vidmoly.",
    "sibnet.",
    "sendvid.",
    "streamtape.",
    "streamhide.",
    "mp4upload.",
    "voe.sx",
    "voe.",
    "doodstream.",
    "mixdrop.",
    "fembed.",
    "kwik.",
    "kwik.cx",
    "my.mail.ru",
    "yourupload.",
    "ok.ru",
    "weneverbeenfree.",
    "filemoon.",
    "uqload.",
    "vidlox.",
    "vupload.",
    "sbembed.",
    "streamz.",
    "smashy.",
    "embed",
];

const AD_HOSTS: &[&str] = &[
    "googlesyndication",
    "doubleclick",
    "googletagmanager",
    "googletagservices",
    "google-analytics",
    "googleadservices",
    "adsystem",
    "adservice",
    "popads",
    "popcash",
    "propellerads",
    "exoclick",
    "adcash",
    "trafficjunky",
    "juicyads",
    "histats",
    "yandex.ru/ads",
    "taboola",
    "outbrain",
    "amazon-adsystem",
    "media.net",
    "criteo",
    "rubiconproject",
    "openx",
    "pubmatic",
    "appnexus",
    "casino",
    "betclic",
    "winamax",
    "betway",
    "1xbet",
    "stake.com",
    "tracker",
    "tracking",
    "trk.",
    "/ads/",
    "/ad/",
    "/track/",
];

fn is_blacklisted_iframe(src: &str) -> bool {
    let lower = src.to_lowercase();
    if lower.contains("voir-anime.to") || lower.contains("about:blank") || lower.is_empty() {
        return true;
    }
    AD_HOSTS.iter().any(|h| lower.contains(h))
}

fn is_whitelisted_video_host(src: &str) -> bool {
    let lower = src.to_lowercase();
    KNOWN_VIDEO_HOSTS.iter().any(|h| lower.contains(h))
}

fn extract_video_frame_block(html: &str) -> Option<&str> {
    let needle = "chapter-video-frame";
    let start = html.find(needle)?;
    let end_search = &html[start..];
    let close = end_search.find("</div>")?;
    Some(&html[start..start + close])
}

fn parse_iframe(html: &str) -> Option<String> {
    let iframe_re = regex::Regex::new(r#"(?is)<iframe[^>]+src=["']([^"']+)["']"#).ok()?;

    if let Some(block) = extract_video_frame_block(html) {
        for cap in iframe_re.captures_iter(block) {
            if let Some(src) = cap.get(1) {
                let src = src.as_str().to_string();
                if !is_blacklisted_iframe(&src) {
                    return Some(src);
                }
            }
        }
    }

    for cap in iframe_re.captures_iter(html) {
        let Some(src) = cap.get(1) else { continue };
        let src = src.as_str().to_string();
        if is_blacklisted_iframe(&src) {
            continue;
        }
        if is_whitelisted_video_host(&src) {
            return Some(src);
        }
    }
    None
}

fn strip_tags(s: &str) -> String {
    let tag_re = regex::Regex::new(r"<[^>]*>").unwrap();
    let no_tags = tag_re.replace_all(s, " ");
    let mut out = String::with_capacity(no_tags.len());
    let mut last_ws = false;
    for c in no_tags.chars() {
        if c.is_whitespace() {
            if !last_ws {
                out.push(' ');
            }
            last_ws = true;
        } else {
            out.push(c);
            last_ws = false;
        }
    }
    out.trim().to_string()
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp_pos) = rest.find('&') {
        out.push_str(&rest[..amp_pos]);
        let after = &rest[amp_pos + 1..];
        let semi = after.find(';');
        let consumed = if let Some(end) = semi {
            if end <= 10 {
                let ent = &after[..end];
                let replacement: Option<String> = match ent {
                    "amp" => Some("&".into()),
                    "lt" => Some("<".into()),
                    "gt" => Some(">".into()),
                    "quot" => Some("\"".into()),
                    "apos" => Some("'".into()),
                    "nbsp" => Some(" ".into()),
                    "hellip" => Some("…".into()),
                    "laquo" => Some("«".into()),
                    "raquo" => Some("»".into()),
                    "eacute" => Some("é".into()),
                    "egrave" => Some("è".into()),
                    "ecirc" => Some("ê".into()),
                    "agrave" => Some("à".into()),
                    "acirc" => Some("â".into()),
                    "ocirc" => Some("ô".into()),
                    "ucirc" => Some("û".into()),
                    "ugrave" => Some("ù".into()),
                    "icirc" => Some("î".into()),
                    "iuml" => Some("ï".into()),
                    "ccedil" => Some("ç".into()),
                    "Eacute" => Some("É".into()),
                    _ => {
                        if let Some(num) = ent.strip_prefix('#') {
                            let code = if let Some(hex) = num
                                .strip_prefix('x')
                                .or_else(|| num.strip_prefix('X'))
                            {
                                u32::from_str_radix(hex, 16).ok()
                            } else {
                                num.parse::<u32>().ok()
                            };
                            code.and_then(char::from_u32).map(|c| c.to_string())
                        } else {
                            None
                        }
                    }
                };
                if let Some(r) = replacement {
                    out.push_str(&r);
                    Some(amp_pos + 1 + end + 1)
                } else {
                    out.push('&');
                    Some(amp_pos + 1)
                }
            } else {
                out.push('&');
                Some(amp_pos + 1)
            }
        } else {
            out.push('&');
            Some(amp_pos + 1)
        };
        rest = &rest[consumed.unwrap()..];
    }
    out.push_str(rest);
    out
}

pub async fn fetch_all_episodes(
    client: &VaClient,
    slug: &str,
) -> Result<VaCachedEpisodes, VaError> {
    let detail = client.anime_detail(slug).await?;
    Ok(VaCachedEpisodes {
        episodes: detail.episodes,
        description: detail.description,
        genres: detail.genres,
        year: detail.year,
        status: detail.status,
        score: detail.score,
        image: detail.image,
        title: detail.title,
    })
}

pub fn va_id_from(slug: &str) -> f64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in slug.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let masked = hash & ((1u64 << 52) - 1);
    let f = masked as f64;
    -(f + 1.0)
}
