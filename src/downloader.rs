use chromiumoxide::cdp::browser_protocol::network::{
    CookieParam, CookiePartitionKey, CookieSameSite, EnableParams, EventResponseReceived,
    TimeSinceEpoch,
};
use chromiumoxide::cdp::browser_protocol::page::NavigateParams;
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use chromiumoxide::js::EvaluationResult;
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, OnceCell, RwLock, Semaphore};

const DEFAULT_CHROME_UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

const MIN_VALID_DOWNLOAD_BYTES: u64 = 100_000;

pub const FRANIME_CF_CLEARANCE_FALLBACK: &str = "XWyNnG2HZ0mdGGKuERIMy7maoxBx_a8Mgxo.4IesuVk-1779817968-1.2.1.1-dUqG8LY74pbxbbt0SDpbEA.guo1IuBxVJlJNwHECNgeHpoNW4ahwTeF.N2Qp6HIQTnngjXn.xdtm4blz0FCXvoGLKFg0V5XFhOWrx8e3FgOzGyxcShCgw1pR4mopjkp99_LrGtqpz0XIdlC8RgPD69ELxptHL35NriIL.E9xz1eV91c3eJ9cVZgHCsqngJxaaGQj1NZZQwNigm_m3ON3OgjpoLzjdst5IVeTGiIFZ.EQE0TlY5iWhlyDYHKz8tQmND2wRd_NyulTqXfKMQeZ0My2P4uhutyqeirUAbnO_62wZSghqWvbXaPTzgSY81VPc28x_3zraTeS9rEZ1LGyBg";

pub const CF_COOKIE_KEY: &str = "cf_clearance";

pub struct CookieStore {
    cookies: tokio::sync::RwLock<std::collections::HashMap<String, String>>,
    user_agent: tokio::sync::RwLock<String>,
    save_tx: mpsc::UnboundedSender<String>,
}

impl CookieStore {
    pub fn new(initial_cf_clearance: String) -> (Arc<Self>, mpsc::UnboundedReceiver<String>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut cookies = std::collections::HashMap::new();
        if !initial_cf_clearance.is_empty() {
            cookies.insert("cf_clearance".to_string(), initial_cf_clearance);
        }
        let store = Arc::new(Self {
            cookies: tokio::sync::RwLock::new(cookies),
            user_agent: tokio::sync::RwLock::new(DEFAULT_CHROME_UA.to_string()),
            save_tx: tx,
        });
        (store, rx)
    }

    pub async fn get(&self) -> String {
        let x= self.cookies
            .read()
            .await
            .get("cf_clearance")
            .cloned()
            .unwrap_or_default();
        println!("cf_clearance: {}", x);
        x
    }

    pub async fn set_all(&self, all: Vec<(String, String)>) {
        let mut map = self.cookies.write().await;
        let mut new_clearance: Option<String> = None;
        for (name, value) in all {
            if name == "cf_clearance" {
                new_clearance = Some(value.clone());
            }
            map.insert(name, value);
        }
        drop(map);
        if let Some(v) = new_clearance {
            let _ = self.save_tx.send(v);
        }
    }

    pub async fn cookie_header(&self) -> String {
        self.cookies
            .read()
            .await
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub async fn user_agent(&self) -> String {
        self.user_agent.read().await.clone()
    }

    pub async fn set_user_agent(&self, ua: String) {
        *self.user_agent.write().await = ua;
    }
}

static IFRAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<iframe[^>]+src=["']([^"']+)["']"#).expect("valid iframe regex")
});

static SIBNET_PLAYER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"player\.src\(\s*\[\s*\{\s*src:\s*["']([^"']+)["']"#)
        .expect("valid sibnet player regex")
});

static SIBNET_FALLBACK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"/v/[A-Za-z0-9_\-/.]+\.mp4"#).expect("valid sibnet fallback regex"));

static SENDVID_SRC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"id=["']video_source["'][^>]*src=["']([^"']+)["']"#)
        .expect("valid sendvid src regex")
});

static SENDVID_SOURCE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<source[^>]+src=["']([^"']+\.mp4[^"']*)["']"#)
        .expect("valid sendvid source regex")
});

static CHROME_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Chrome/(\d+)\.").expect("valid chrome version regex"));

fn sec_ch_ua_for(ua: &str) -> String {
    let v = CHROME_VERSION_RE
        .captures(ua)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
        .unwrap_or("148");
    format!(
        r#""Google Chrome";v="{}", "Chromium";v="{}", "Not_A Brand";v="24""#,
        v, v
    )
}

async fn eval_stealth(page: &Page, script: &str) -> Result<EvaluationResult> {
    let params = EvaluateParams::builder()
        .expression(script)
        .user_gesture(true)
        .allow_unsafe_eval_blocked_by_csp(true)
        .return_by_value(true)
        .build()
        .map_err(|e| ScraperError::Navigation(format!("EvaluateParams: {}", e)))?;
    page.evaluate(params)
        .await
        .map_err(|e| ScraperError::Navigation(e.to_string()))
}

fn is_cloudflare_challenge(html: &str) -> bool {
    html.contains("Just a moment")
        || html.contains("challenges.cloudflare.com")
        || html.contains("cf-challenge-running")
        || html.contains("cf_chl_opt")
}

fn is_known_video_host(src: &str) -> bool {
    let s = src.to_lowercase();
    const HOSTS: &[&str] = &[
        "sibnet.ru",
        "sendvid.com",
        "filemoon",
        "vidmoly",
        "mixdrop",
        "streamtape",
        "streamta.pe",
        "doodstream",
        "dood.",
        "upstream",
        "vudeo",
        "okru",
        "ok.ru",
        "mp4upload",
        "voe.sx",
        "vk.com",
        "streamwish",
        "streamhide",
        "mytv",
        "lulu.st",
        "embedsito",
    ];
    HOSTS.iter().any(|h| s.contains(h))
}

fn classify_provider(iframe_src: &str) -> VideoProvider {
    if iframe_src.contains("sibnet.ru") {
        VideoProvider::Sibnet
    } else if iframe_src.contains("sendvid.com") {
        VideoProvider::Sendvid
    } else if iframe_src.contains("filemoon") {
        VideoProvider::FileMoon
    } else if iframe_src.contains("vidmoly") {
        VideoProvider::Vidmoly
    } else {
        VideoProvider::Unknown
    }
}

static VIDMOLY_SOURCE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"sources\s*:\s*\[\s*\{\s*file\s*:\s*["']([^"']+)["']"#)
        .expect("valid vidmoly regex")
});
static JW_FILE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"["']?file["']?\s*:\s*["'](https?://[^"']+|//[^"']+)["']"#)
        .expect("valid jwplayer file regex")
});

const HTTP_SCRAPE_HOSTS: &[&str] = &[
    "vidmoly",
    "yourupload",
    "mp4upload",
    "vudeo",
    "upstream",
    "vupload",
    "uqload",
];
static GENERIC_MP4_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(https?://[^"'\s<>]+\.(?:mp4|m3u8)(?:\?[^"'\s<>]*)?)"#)
        .expect("valid generic mp4 regex")
});

#[derive(Debug)]
enum HttpAttempt {
    Found(String),
    Challenge,
    Miss,
}

struct HttpExtractor {
    client: wreq::Client,
    cookies: Arc<CookieStore>,
}

impl HttpExtractor {
    fn new(cookies: Arc<CookieStore>) -> Self {
        let client = wreq::Client::builder()
            .emulation(wreq_util::Emulation::Safari26_2)
            .timeout(Duration::from_secs(20))
            .build()
            .expect("Failed to build wreq HTTP extractor client");
        Self { client, cookies }
    }

    async fn try_iframe_src(&self, url: &str) -> HttpAttempt {
        let cookie_header = self.cookies.cookie_header().await;
        let ua = self.cookies.user_agent().await;
        crate::applog::log_event(
            crate::applog::LogSource::App,
            crate::applog::LogLevel::Info,
            format!("HTTP GET {} cookie_len={}", url, cookie_header.len()),
        );
        let resp = match self
            .client
            .get(url)
            .header("User-Agent", &ua)
            .header("Cookie", cookie_header)
            .header("Referer", "https://franime.fr/")
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8")
            .header("Accept-Language", "fr-FR,fr;q=0.9,en-US;q=0.8,en;q=0.7")
            .header("Accept-Encoding", "gzip, deflate, br, zstd")
            .header("Sec-Ch-Ua", sec_ch_ua_for(&ua))
            .header("Sec-Ch-Ua-Mobile", "?0")
            .header("Sec-Ch-Ua-Platform", "\"macOS\"")
            .header("Sec-Fetch-Dest", "document")
            .header("Sec-Fetch-Mode", "navigate")
            .header("Sec-Fetch-Site", "same-origin")
            .header("Sec-Fetch-User", "?1")
            .header("Upgrade-Insecure-Requests", "1")
            .header("Priority", "u=0, i")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                crate::applog::log_event(
                    crate::applog::LogSource::App,
                    crate::applog::LogLevel::Warn,
                    format!("HTTP error: {}", e),
                );
                return HttpAttempt::Miss;
            }
        };

        let status = resp.status();
        let cf_ray = resp.headers().get("cf-ray").is_some();
        crate::applog::log_event(
            crate::applog::LogSource::App,
            crate::applog::LogLevel::Info,
            format!("HTTP status={} cf-ray={}", status, cf_ray),
        );

        let html = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                crate::applog::log_event(
                    crate::applog::LogSource::App,
                    crate::applog::LogLevel::Warn,
                    format!("HTTP body error: {}", e),
                );
                return HttpAttempt::Miss;
            }
        };

        if is_cloudflare_challenge(&html)
            || (cf_ray && matches!(status.as_u16(), 403 | 429 | 503))
        {
            crate::applog::log_event(
                crate::applog::LogSource::App,
                crate::applog::LogLevel::Warn,
                format!("CF challenge detected (status={} len={})", status, html.len()),
            );
            return HttpAttempt::Challenge;
        }

        if !status.is_success() {
            return HttpAttempt::Miss;
        }

        match IFRAME_RE
            .captures_iter(&html)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .filter(|src| is_known_video_host(src))
            .last()
        {
            Some(s) => HttpAttempt::Found(s),
            None => HttpAttempt::Miss,
        }
    }

    async fn try_sibnet(&self, iframe_src: &str) -> Option<String> {
        let ua = self.cookies.user_agent().await;
        let resp = self
            .client
            .get(iframe_src)
            .header("User-Agent", ua)
            .header("Referer", "https://video.sibnet.ru/")
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let html = resp.text().await.ok()?;

        let path = SIBNET_PLAYER_RE
            .captures(&html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .or_else(|| {
                SIBNET_FALLBACK_RE
                    .find(&html)
                    .map(|m| m.as_str().to_string())
            })?;

        Some(if path.starts_with("http") {
            path
        } else if path.starts_with("//") {
            format!("https:{}", path)
        } else {
            format!("https://video.sibnet.ru{}", path)
        })
    }

    async fn try_vidmoly(&self, iframe_src: &str) -> Option<String> {
        let ua = self.cookies.user_agent().await;
        let resp = self
            .client
            .get(iframe_src)
            .header("User-Agent", ua)
            .header("Referer", "https://franime.fr/")
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let html = resp.text().await.ok()?;
        if let Some(c) = VIDMOLY_SOURCE_RE.captures(&html) {
            if let Some(m) = c.get(1) {
                return Some(m.as_str().to_string());
            }
        }
        GENERIC_MP4_RE
            .find(&html)
            .map(|m| m.as_str().to_string())
    }

    async fn try_http_scrape(&self, url: &str, referer: &str) -> Option<String> {
        let ua = self.cookies.user_agent().await;
        let mut req = self.client.get(url).header("User-Agent", ua);
        if !referer.is_empty() {
            req = req.header("Referer", referer);
        }
        let resp = req.send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let html = resp.text().await.ok()?;
        if let Some(c) = VIDMOLY_SOURCE_RE.captures(&html) {
            if let Some(m) = c.get(1) {
                return Some(normalize_scheme(m.as_str()));
            }
        }
        if let Some(c) = JW_FILE_RE.captures(&html) {
            if let Some(m) = c.get(1) {
                let v = m.as_str();
                if v.contains(".mp4") || v.contains(".m3u8") {
                    return Some(normalize_scheme(v));
                }
            }
        }
        GENERIC_MP4_RE
            .find(&html)
            .map(|m| normalize_scheme(m.as_str()))
    }

    async fn try_sendvid(&self, iframe_src: &str) -> Option<String> {
        let ua = self.cookies.user_agent().await;
        let resp = self
            .client
            .get(iframe_src)
            .header("User-Agent", ua)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let html = resp.text().await.ok()?;

        let candidate = SENDVID_SRC_RE
            .captures(&html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .or_else(|| {
                SENDVID_SOURCE_RE
                    .captures(&html)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().to_string())
            })?;

        if candidate == "undefined" || candidate.is_empty() {
            return None;
        }
        Some(candidate)
    }
}

#[derive(Debug, Clone)]
pub enum ScraperError {
    BrowserLaunch(String),
    Navigation(String),
    VideoSourceNotFound,
    Timeout(String),
    UnsupportedProvider(String),
    IoError(String),
    NetworkError(String),
}

impl std::fmt::Display for ScraperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BrowserLaunch(msg) => write!(f, "Erreur de lancement du navigateur: {}", msg),
            Self::Navigation(msg) => write!(f, "Erreur de navigation: {}", msg),
            Self::VideoSourceNotFound => write!(f, "Source vidéo non trouvée"),
            Self::Timeout(msg) => write!(f, "Timeout: {}", msg),
            Self::UnsupportedProvider(url) => write!(f, "Provider non supporté: {}", url),
            Self::IoError(msg) => write!(f, "Erreur IO: {}", msg),
            Self::NetworkError(msg) => write!(f, "Erreur réseau: {}", msg),
        }
    }
}

impl std::error::Error for ScraperError {}

pub type Result<T> = std::result::Result<T, ScraperError>;

#[derive(Debug, Clone)]
pub struct VideoSource {
    pub url: String,
    pub provider: VideoProvider,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VideoProvider {
    Sibnet,
    Sendvid,
    FileMoon,
    Vidmoly,
    Unknown,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DownloadProgress {
    pub id: String,
    pub downloaded: u64,
    pub total: u64,
    pub percentage: f32,
    pub speed_bytes_per_sec: u64,
    pub eta_seconds: u64,
    pub resolution: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum DownloadStatus {
    Queued,
    Extracting,
    Downloading(DownloadProgress),
    Completed,
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub id: String,
    pub url: String,
    pub output_path: PathBuf,
    pub status: DownloadStatus,
    pub host: Option<String>,
    pub attempted_lecteurs: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 0,
            initial_delay_ms: 1000,
            max_delay_ms: 10000,
            backoff_multiplier: 2.0,
        }
    }
}

pub struct FranimeScraper {
    browser: OnceCell<Browser>,
    http: HttpExtractor,
    cookies: Arc<CookieStore>,
    sidecar: Arc<crate::cf_sidecar::Sidecar>,
    refresh_lock: tokio::sync::Mutex<()>,
    pub cf_refreshing: Arc<std::sync::atomic::AtomicBool>,
    retry_config: RetryConfig,
    headless: bool,
}

impl FranimeScraper {
    pub fn new(headless: bool, cookies: Arc<CookieStore>) -> Self {
        Self::new_with_retry(headless, RetryConfig::default(), cookies)
    }

    pub fn new_with_retry(
        headless: bool,
        retry_config: RetryConfig,
        cookies: Arc<CookieStore>,
    ) -> Self {
        Self {
            browser: OnceCell::new(),
            http: HttpExtractor::new(cookies.clone()),
            cookies,
            sidecar: crate::cf_sidecar::Sidecar::new(headless),
            refresh_lock: tokio::sync::Mutex::new(()),
            cf_refreshing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            retry_config,
            headless,
        }
    }

    async fn ensure_sidecar(&self) -> Result<()> {
        if self.sidecar.is_alive().await {
            return Ok(());
        }
        self.cf_refreshing
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let ready_result = self.sidecar.ensure_started().await;
        self.cf_refreshing
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let ready = ready_result.map_err(|e| {
            crate::applog::log_event(
                crate::applog::LogSource::Sidecar,
                crate::applog::LogLevel::Error,
                format!("start failed: {}", e),
            );
            ScraperError::Navigation(format!("sidecar nodriver: {}", e))
        })?;
        crate::applog::log_event(
            crate::applog::LogSource::Sidecar,
            crate::applog::LogLevel::Info,
            format!(
                "ready ua={} cookies={}",
                &ready.user_agent[..30.min(ready.user_agent.len())],
                ready.all_cookies.len()
            ),
        );
        if !ready.user_agent.is_empty() {
            self.cookies.set_user_agent(ready.user_agent).await;
        }
        let cookies: Vec<(String, String)> = ready.all_cookies.into_iter().collect();
        self.cookies.set_all(cookies).await;
        Ok(())
    }

    pub fn sidecar(&self) -> Arc<crate::cf_sidecar::Sidecar> {
        self.sidecar.clone()
    }

    async fn browser(&self) -> Result<&Browser> {
        self.browser
            .get_or_try_init(|| async {
                let profile_dir = std::env::temp_dir()
                    .join(format!("uc_{}", uuid::Uuid::new_v4().simple()));
                let _ = fs::create_dir_all(&profile_dir);

                let mut builder = BrowserConfig::builder()
                    .disable_default_args()
                    .user_data_dir(&profile_dir)
                    .arg("--remote-allow-origins=*")
                    .arg("--no-first-run")
                    .arg("--no-service-autorun")
                    .arg("--no-default-browser-check")
                    .arg("--homepage=about:blank")
                    .arg("--no-pings")
                    .arg("--password-store=basic")
                    .arg("--disable-infobars")
                    .arg("--disable-breakpad")
                    .arg("--disable-dev-shm-usage")
                    .arg("--disable-session-crashed-bubble")
                    .arg("--disable-search-engine-choice-screen")
                    .arg("--disable-features=IsolateOrigins,site-per-process");

                if !self.headless {
                    builder = builder.with_head();
                }
                let config = builder
                    .build()
                    .map_err(ScraperError::BrowserLaunch)?;

                let (browser, mut handler) = Browser::launch(config)
                    .await
                    .map_err(|e| ScraperError::BrowserLaunch(e.to_string()))?;

                tokio::spawn(async move { while handler.next().await.is_some() {} });

                if let Ok(ua) = browser.user_agent().await {
                    self.cookies.set_user_agent(ua).await;
                }

                let cookie_value = self.cookies.get().await;
                if !cookie_value.is_empty() {
                    let cookie = CookieParam::builder()
                        .expires(TimeSinceEpoch::new(1799665392.0))
                        .secure(true)
                        .http_only(true)
                        .same_site(CookieSameSite::None)
                        .domain(".franime.fr")
                        .path("/")
                        .name("cf_clearance")
                        .value(cookie_value)
                        .partition_key(CookiePartitionKey::new("https://franime.fr", false))
                        .build()
                        .map_err(|e| ScraperError::BrowserLaunch(e.to_string()))?;

                    browser
                        .set_cookies(vec![cookie])
                        .await
                        .map_err(|e| ScraperError::BrowserLaunch(e.to_string()))?;
                }

                Ok(browser)
            })
            .await
    }

    pub async fn refresh_cf_cookie(&self) -> Result<()> {
        let initial = self.cookies.get().await;
        let _guard = self.refresh_lock.lock().await;

        if self.cookies.get().await != initial {
            return Ok(());
        }

        self.cf_refreshing.store(true, std::sync::atomic::Ordering::SeqCst);
        let result = self.do_refresh(initial).await;
        self.cf_refreshing.store(false, std::sync::atomic::Ordering::SeqCst);
        result
    }

    async fn do_refresh(&self, _initial: String) -> Result<()> {
        self.ensure_sidecar().await?;
        let ready = self.sidecar.refresh_cf().await.map_err(|e| {
            crate::applog::log_event(
                crate::applog::LogSource::Sidecar,
                crate::applog::LogLevel::Error,
                format!("refresh_cf failed: {}", e),
            );
            ScraperError::Navigation(format!("sidecar refresh_cf: {}", e))
        })?;
        if !ready.user_agent.is_empty() {
            self.cookies.set_user_agent(ready.user_agent).await;
        }
        let cookies: Vec<(String, String)> = ready.all_cookies.into_iter().collect();
        self.cookies.set_all(cookies).await;
        Ok(())
    }

    pub async fn extract_video_source(&self, url: &str) -> Result<VideoSource> {
        self.retry_operation(|| self.extract_video_source_impl(url))
            .await
    }

    async fn extract_video_source_impl(&self, url: &str) -> Result<VideoSource> {
        let iframe_src = match self.http.try_iframe_src(url).await {
            HttpAttempt::Found(s) => s,
            HttpAttempt::Challenge | HttpAttempt::Miss => {
                self.browser_iframe_src(url).await?
            }
        };

        let provider = classify_provider(&iframe_src);

        let video_url = match provider {
            VideoProvider::Sibnet => match self.http.try_sibnet(&iframe_src).await {
                Some(u) => u,
                None => match self.http.try_vidmoly(&iframe_src).await {
                    Some(u) => u,
                    None => self.extract_via_browser_or_ytdlp(&iframe_src).await?,
                },
            },
            VideoProvider::Sendvid => match self.http.try_sendvid(&iframe_src).await {
                Some(u) => u,
                None => self.extract_via_browser_or_ytdlp(&iframe_src).await?,
            },
            VideoProvider::FileMoon => self.extract_via_browser_or_ytdlp(&iframe_src).await?,
            VideoProvider::Vidmoly => match self.http.try_vidmoly(&iframe_src).await {
                Some(u) => u,
                None => self.extract_via_browser_or_ytdlp(&iframe_src).await?,
            },
            VideoProvider::Unknown => {
                crate::applog::log_event(
                    crate::applog::LogSource::App,
                    crate::applog::LogLevel::Info,
                    format!("Host inconnu, fallback sidecar: {}", iframe_src),
                );
                self.extract_via_browser_or_ytdlp(&iframe_src).await?
            }
        };

        Ok(VideoSource {
            url: video_url,
            provider,
        })
    }

    pub async fn extract_video_source_from_embed(
        &self,
        embed_url: &str,
        referer: &str,
    ) -> Result<VideoSource> {
        self.retry_operation(|| self.extract_embed_impl(embed_url, referer))
            .await
    }

    async fn extract_embed_impl(&self, embed_url: &str, referer: &str) -> Result<VideoSource> {
        let provider = classify_provider(embed_url);
        let lower = embed_url.to_lowercase();
        crate::applog::log_event(
            crate::applog::LogSource::App,
            crate::applog::LogLevel::Info,
            format!("embed direct: {} (ref={})", embed_url, referer),
        );

        let make = |url: String| VideoSource {
            url,
            provider,
        };

        if lower.contains("sibnet.ru") {
            if let Some(u) = self.http.try_sibnet(embed_url).await {
                return Ok(make(u));
            }
        } else if lower.contains("sendvid") {
            if let Some(u) = self.http.try_sendvid(embed_url).await {
                return Ok(make(u));
            }
        } else if HTTP_SCRAPE_HOSTS.iter().any(|h| lower.contains(h)) {
            if let Some(u) = self.http.try_http_scrape(embed_url, referer).await {
                crate::applog::log_event(
                    crate::applog::LogSource::App,
                    crate::applog::LogLevel::Info,
                    format!("embed HTTP scrape OK: {}", &u[..u.len().min(120)]),
                );
                return Ok(make(u));
            }
        }

        match crate::ytdlp::extract_url(embed_url, referer).await {
            Ok(u) if !u.is_empty() => {
                crate::applog::log_event(
                    crate::applog::LogSource::App,
                    crate::applog::LogLevel::Info,
                    format!("embed yt-dlp OK: {}", &u[..u.len().min(120)]),
                );
                return Ok(make(u));
            }
            Ok(_) => {}
            Err(e) => {
                crate::applog::log_event(
                    crate::applog::LogSource::App,
                    crate::applog::LogLevel::Info,
                    format!("embed yt-dlp KO ({}), fallback sidecar", e),
                );
            }
        }

        match self.sidecar_capture(embed_url, Some(referer)).await {
            Ok(u) if !u.is_empty() => {
                crate::applog::log_event(
                    crate::applog::LogSource::App,
                    crate::applog::LogLevel::Info,
                    format!("embed sidecar capture OK: {}", &u[..u.len().min(120)]),
                );
                Ok(make(u))
            }
            Ok(_) => Err(ScraperError::VideoSourceNotFound),
            Err(e) => Err(e),
        }
    }

    async fn browser_iframe_src(&self, url: &str) -> Result<String> {
        self.ensure_sidecar().await?;
        self.sidecar.fetch_iframe(url).await.map_err(|e| {
            crate::applog::log_event(
                crate::applog::LogSource::Sidecar,
                crate::applog::LogLevel::Error,
                format!("fetch_iframe failed: {}", e),
            );
            ScraperError::Navigation(format!("sidecar fetch_iframe: {}", e))
        })
    }

    async fn extract_via_browser_or_ytdlp(&self, iframe_src: &str) -> Result<String> {
        self.extract_via_browser_or_ytdlp_ref(iframe_src, "https://franime.fr/")
            .await
    }

    async fn extract_via_browser_or_ytdlp_ref(
        &self,
        iframe_src: &str,
        referer: &str,
    ) -> Result<String> {
        match self.sidecar_capture(iframe_src, Some(referer)).await {
            Ok(u) if !u.is_empty() => return Ok(u),
            Ok(_) => {
                crate::applog::log_event(
                    crate::applog::LogSource::App,
                    crate::applog::LogLevel::Warn,
                    "sidecar_capture vide, tentative yt-dlp",
                );
            }
            Err(e) => {
                crate::applog::log_event(
                    crate::applog::LogSource::App,
                    crate::applog::LogLevel::Warn,
                    format!("sidecar_capture KO ({}), tentative yt-dlp", e),
                );
            }
        }
        crate::applog::log_event(
            crate::applog::LogSource::App,
            crate::applog::LogLevel::Info,
            format!("yt-dlp sur {}", iframe_src),
        );
        match crate::ytdlp::extract_url(iframe_src, referer).await {
            Ok(u) => {
                crate::applog::log_event(
                    crate::applog::LogSource::App,
                    crate::applog::LogLevel::Info,
                    format!("yt-dlp OK: {}", &u[..u.len().min(120)]),
                );
                Ok(u)
            }
            Err(e) => Err(ScraperError::Navigation(format!("yt-dlp: {}", e))),
        }
    }

    async fn sidecar_capture(&self, iframe_src: &str, referer: Option<&str>) -> Result<String> {
        self.ensure_sidecar().await?;
        self.sidecar
            .capture_video_url(iframe_src, referer)
            .await
            .map_err(|e| {
                crate::applog::log_event(
                    crate::applog::LogSource::Sidecar,
                    crate::applog::LogLevel::Error,
                    format!("capture_video_url failed: {}", e),
                );
                ScraperError::Navigation(format!("sidecar capture: {}", e))
            })
    }

    async fn wait_for_provider_iframe(&self, page: &Page) -> Result<String> {
        let timeout = Duration::from_secs(30);
        let start = tokio::time::Instant::now();

        loop {
            if start.elapsed() > timeout {
                return Err(ScraperError::Timeout(
                    "Attente d'une iframe provider".to_string(),
                ));
            }

            let script = r#"
                (() => {
                    const frames = Array.from(document.querySelectorAll('iframe'));
                    const matched = frames
                        .map(i => i.src || i.getAttribute('src') || '')
                        .filter(s => s && (
                            s.includes('sibnet.ru') ||
                            s.includes('sendvid.com') ||
                            s.includes('filemoon')
                        ));
                    return matched.length > 0 ? matched[matched.length - 1] : '';
                })()
            "#;

            if let Ok(result) = eval_stealth(page, script).await {
                if let Ok(src) = result.into_value::<String>() {
                    if !src.is_empty() {
                        return Ok(src);
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    async fn browser_sibnet(&self, iframe_src: &str) -> Result<String> {
        let page = self.fresh_page().await?;
        self.extract_sibnet_url(&page, iframe_src).await
    }

    async fn browser_sendvid(&self, iframe_src: &str) -> Result<String> {
        let page = self.fresh_page().await?;
        self.extract_sendvid_url(&page, iframe_src).await
    }

    async fn browser_filemoon(&self, iframe_src: &str) -> Result<String> {
        let page = self.fresh_page().await?;
        self.extract_filemoon_url(&page, iframe_src).await
    }

    async fn fresh_page(&self) -> Result<Page> {
        let browser = self.browser().await?;
        browser
            .new_page("about:blank")
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))
    }

    async fn retry_operation<F, Fut, T>(&self, mut operation: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut attempts = 0;
        let mut delay = self.retry_config.initial_delay_ms;

        loop {
            match operation().await {
                Ok(result) => return Ok(result),
                Err(e) if attempts >= self.retry_config.max_retries => {
                    return Err(e);
                }
                Err(e) => {
                    attempts += 1;
                    eprintln!(
                        "Tentative {}/{} échouée: {}. Nouvelle tentative dans {}ms...",
                        attempts,
                        self.retry_config.max_retries + 1,
                        e,
                        delay
                    );

                    tokio::time::sleep(Duration::from_millis(delay)).await;

                    delay = (delay as f32 * self.retry_config.backoff_multiplier) as u64;
                    delay = delay.min(self.retry_config.max_delay_ms);
                }
            }
        }
    }

    async fn extract_sibnet_url(&self, page: &Page, iframe_src: &str) -> Result<String> {
        let video_page = page
            .goto(
                NavigateParams::builder()
                    .url(iframe_src)
                    .build()
                    .map_err(|e| ScraperError::Navigation(e.to_string()))?,
            )
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?
            .wait_for_navigation()
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;

        video_page
            .execute(EnableParams::default())
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;
        let mut net_events = video_page
            .event_listener::<EventResponseReceived>()
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;

        eval_stealth(
            &video_page,
            r#"
            (() => {
                const v = document.querySelector('video');
                if (!v) return "NO_VIDEO";
                v.muted = true;
                v.play();
                return "PLAYING";
            })()
            "#,
        )
        .await?;

        let timeout = tokio::time::Duration::from_secs(15);
        let start = tokio::time::Instant::now();

        while let Some(ev) = net_events.next().await {
            if start.elapsed() > timeout {
                return Err(ScraperError::Timeout("Attente de l'URL vidéo".to_string()));
            }

            let res = &ev.response;

            if res.mime_type == "video/mp4" && res.url.contains("sibnet.ru") {
                eval_stealth(
                    &video_page,
                    r#"
                    (() => {
                        const v = document.querySelector('video');
                        if (v) v.pause();
                    })()
                    "#,
                )
                .await?;

                return Ok(res.url.clone());
            }
        }

        Err(ScraperError::VideoSourceNotFound)
    }

    async fn extract_sendvid_url(&self, page: &Page, iframe_src: &str) -> Result<String> {
        let video_page = page
            .goto(
                NavigateParams::builder()
                    .url(iframe_src)
                    .build()
                    .map_err(|e| ScraperError::Navigation(e.to_string()))?,
            )
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?
            .wait_for_navigation()
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;

        let timeout = Duration::from_secs(15);
        let start = tokio::time::Instant::now();
        loop {
            if start.elapsed() > timeout {
                return Err(ScraperError::Timeout(
                    "Attente de la source sendvid".to_string(),
                ));
            }

            let script = r#"
                (() => {
                    const el = document.getElementById('video_source')
                        || document.querySelector('source[src]');
                    if (!el) return '';
                    const s = el.getAttribute('src') || '';
                    return (s && s !== 'undefined') ? s : '';
                })()
            "#;

            if let Ok(result) = eval_stealth(&video_page, script).await {
                if let Ok(src) = result.into_value::<String>() {
                    if !src.is_empty() {
                        return Ok(src);
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn extract_filemoon_url(&self, page: &Page, iframe_src: &str) -> Result<String> {
        let video_page = page
            .goto(
                NavigateParams::builder()
                    .url(iframe_src)
                    .build()
                    .map_err(|e| ScraperError::Navigation(e.to_string()))?,
            )
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?
            .wait_for_navigation()
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;

        video_page
            .execute(EnableParams::default())
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;
        let mut net_events = video_page
            .event_listener::<EventResponseReceived>()
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;

        let timeout = tokio::time::Duration::from_secs(50);
        let start = tokio::time::Instant::now();
        while let Some(ev) = net_events.next().await {
            if start.elapsed() > timeout {
                return Err(ScraperError::Timeout("Attente de l'URL vidéo".to_string()));
            }

            let res = &ev.response;

            if res.mime_type.is_empty() && res.url.contains("filemoon.to") {
                return Ok(res.url.clone());
            }
        }

        Err(ScraperError::VideoSourceNotFound)
    }
}

pub fn sanitize_path_segment(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    if trimmed.is_empty() {
        "_".to_string()
    } else {
        trimmed.to_string()
    }
}

#[derive(Debug, Clone)]
pub enum DownloadEvent {
    Updated(DownloadTask),
    Removed(String),
}

pub struct DownloadManager {
    scraper: Arc<FranimeScraper>,
    downloader: Arc<VideoDownloader>,
    tasks: Arc<RwLock<Vec<DownloadTask>>>,
    semaphore: Arc<Semaphore>,
    update_tx: mpsc::UnboundedSender<DownloadEvent>,
    cancel_signals:
        Arc<tokio::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Notify>>>>,
}

impl DownloadManager {
    pub fn new(
        headless: bool,
        max_concurrent: usize,
        tasks: Arc<RwLock<Vec<DownloadTask>>>,
        cookies: Arc<CookieStore>,
    ) -> (Self, mpsc::UnboundedReceiver<DownloadEvent>) {
        let scraper = Arc::new(FranimeScraper::new(headless, cookies));
        let downloader = Arc::new(VideoDownloader::new());
        let semaphore = Arc::new(Semaphore::new(max_concurrent.max(1)));
        let (update_tx, update_rx) = mpsc::unbounded_channel();

        (
            Self {
                scraper,
                downloader,
                tasks,
                semaphore,
                update_tx,
                cancel_signals: Arc::new(tokio::sync::Mutex::new(
                    std::collections::HashMap::new(),
                )),
            },
            update_rx,
        )
    }

    pub fn sidecar(&self) -> Arc<crate::cf_sidecar::Sidecar> {
        self.scraper.sidecar()
    }

    pub async fn set_task_host(&self, id: &str, host: Option<String>) {
        let mut tasks = self.tasks.write().await;
        if let Some(t) = tasks.iter_mut().find(|t| t.id == id) {
            t.host = host;
            let _ = self.update_tx.send(DownloadEvent::Updated(t.clone()));
        }
    }

    pub async fn add_attempted_lecteur(&self, id: &str, lecteur: u64) {
        let mut tasks = self.tasks.write().await;
        if let Some(t) = tasks.iter_mut().find(|t| t.id == id) {
            if !t.attempted_lecteurs.contains(&lecteur) {
                t.attempted_lecteurs.push(lecteur);
            }
            let _ = self.update_tx.send(DownloadEvent::Updated(t.clone()));
        }
    }

    async fn cancel_signal_for(&self, id: &str) -> Arc<tokio::sync::Notify> {
        let mut map = self.cancel_signals.lock().await;
        map.entry(id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Notify::new()))
            .clone()
    }

    async fn forget_cancel_signal(&self, id: &str) {
        self.cancel_signals.lock().await.remove(id);
    }

    pub async fn warmup_sidecar(&self) -> Result<()> {
        self.scraper.ensure_sidecar().await
    }

    pub fn cf_refreshing(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.scraper.cf_refreshing.clone()
    }

    pub async fn add_pending(&self, output_path: PathBuf) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let task = DownloadTask {
            id: id.clone(),
            url: String::new(),
            output_path,
            status: DownloadStatus::Extracting,
            host: None,
            attempted_lecteurs: Vec::new(),
        };
        self.tasks.write().await.push(task.clone());
        let _ = self.update_tx.send(DownloadEvent::Updated(task));
        id
    }

    pub async fn extract_and_download(
        &self,
        id: String,
        iframe_url: String,
    ) -> Result<()> {
        {
            let mut tasks = self.tasks.write().await;
            match tasks.iter_mut().find(|t| t.id == id) {
                Some(t) => {
                    t.url = iframe_url.clone();
                    t.status = DownloadStatus::Extracting;
                    let _ = self.update_tx.send(DownloadEvent::Updated(t.clone()));
                }
                None => {
                    return Err(ScraperError::IoError(format!("Task {} introuvable", id)));
                }
            }
        }

        if self.is_cancelled(&id).await {
            return Ok(());
        }

        let source = self.scraper.extract_video_source(&iframe_url).await?;
        self.download_source(id, source).await
    }

    pub async fn extract_and_download_embed(
        &self,
        id: String,
        embed_url: String,
        referer: String,
    ) -> Result<()> {
        {
            let mut tasks = self.tasks.write().await;
            match tasks.iter_mut().find(|t| t.id == id) {
                Some(t) => {
                    t.url = embed_url.clone();
                    t.status = DownloadStatus::Extracting;
                    let _ = self.update_tx.send(DownloadEvent::Updated(t.clone()));
                }
                None => {
                    return Err(ScraperError::IoError(format!("Task {} introuvable", id)));
                }
            }
        }
        if self.is_cancelled(&id).await {
            return Ok(());
        }
        let source = self
            .scraper
            .extract_video_source_from_embed(&embed_url, &referer)
            .await?;
        self.download_source(id, source).await
    }

    async fn download_source(&self, id: String, source: VideoSource) -> Result<()> {
        if self.is_cancelled(&id).await {
            return Ok(());
        }

        let output = match self.tasks.read().await.iter().find(|t| t.id == id).map(|t| t.output_path.clone()) {
            Some(p) => p,
            None => return Err(ScraperError::IoError(format!("Task {} disparue", id))),
        };

        self.update_task_status(&id, DownloadStatus::Queued).await;
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| ScraperError::IoError(e.to_string()))?;

        if self.is_cancelled(&id).await {
            return Ok(());
        }

        let id_for_callback = id.clone();
        let update_tx = self.update_tx.clone();
        let output_for_callback = output.clone();
        let url_for_callback = source.url.clone();

        let cancel = self.cancel_signal_for(&id).await;
        let result = self
            .downloader
            .download(
                &source,
                &output,
                Some(Box::new(move |progress| {
                    let task = DownloadTask {
                        id: id_for_callback.clone(),
                        url: url_for_callback.clone(),
                        output_path: output_for_callback.clone(),
                        status: DownloadStatus::Downloading(progress),
                        host: None,
                        attempted_lecteurs: Vec::new(),
                    };
                    let _ = update_tx.send(DownloadEvent::Updated(task));
                })),
                Some(cancel),
            )
            .await;

        match result {
            Ok(_) => {
                if !self.is_cancelled(&id).await {
                    self.update_task_status(&id, DownloadStatus::Completed).await;
                }
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub async fn mark_failed(&self, id: &str, msg: String) {
        self.update_task_status(id, DownloadStatus::Failed(msg)).await;
    }

    pub async fn download_direct(&self, id: String, video_url: String) -> Result<()> {
        {
            let mut tasks = self.tasks.write().await;
            match tasks.iter_mut().find(|t| t.id == id) {
                Some(t) => {
                    t.url = video_url.clone();
                    t.status = DownloadStatus::Queued;
                    let _ = self.update_tx.send(DownloadEvent::Updated(t.clone()));
                }
                None => {
                    return Err(ScraperError::IoError(format!("Task {} introuvable", id)));
                }
            }
        }
        if self.is_cancelled(&id).await {
            return Ok(());
        }
        let output = match self
            .tasks
            .read()
            .await
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.output_path.clone())
        {
            Some(p) => p,
            None => return Err(ScraperError::IoError(format!("Task {} disparue", id))),
        };
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| ScraperError::IoError(e.to_string()))?;
        if self.is_cancelled(&id).await {
            return Ok(());
        }
        let source = VideoSource {
            url: video_url.clone(),
            provider: VideoProvider::Unknown,
        };
        let id_for_callback = id.clone();
        let update_tx = self.update_tx.clone();
        let output_for_callback = output.clone();
        let url_for_callback = video_url.clone();
        let cancel = self.cancel_signal_for(&id).await;
        let result = self
            .downloader
            .download(
                &source,
                &output,
                Some(Box::new(move |progress| {
                    let task = DownloadTask {
                        id: id_for_callback.clone(),
                        url: url_for_callback.clone(),
                        output_path: output_for_callback.clone(),
                        status: DownloadStatus::Downloading(progress),
                        host: None,
                        attempted_lecteurs: Vec::new(),
                    };
                    let _ = update_tx.send(DownloadEvent::Updated(task));
                })),
                Some(cancel),
            )
            .await;
        match result {
            Ok(_) => {
                if !self.is_cancelled(&id).await {
                    self.update_task_status(&id, DownloadStatus::Completed).await;
                }
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub async fn cancel(&self, id: &str) {
        if let Some(notify) = self.cancel_signals.lock().await.get(id).cloned() {
            notify.notify_waiters();
        }
        self.update_task_status(id, DownloadStatus::Cancelled).await;
    }

    pub async fn forget(&self, id: &str) {
        let mut tasks = self.tasks.write().await;
        tasks.retain(|t| t.id != id);
        let _ = self.update_tx.send(DownloadEvent::Removed(id.to_string()));
    }

    async fn update_task_status(&self, id: &str, status: DownloadStatus) {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.iter_mut().find(|t| t.id == id) {
            task.status = status;
            let _ = self.update_tx.send(DownloadEvent::Updated(task.clone()));
        }
    }

    async fn is_cancelled(&self, id: &str) -> bool {
        let tasks = self.tasks.read().await;
        tasks
            .iter()
            .find(|t| t.id == id)
            .map(|t| matches!(t.status, DownloadStatus::Cancelled))
            .unwrap_or(false)
    }
}

pub type ProgressCallback = Box<dyn Fn(DownloadProgress) + Send + Sync>;

pub struct VideoDownloader {
    client: reqwest::Client,
    retry_config: RetryConfig,
}

impl VideoDownloader {
    pub fn new() -> Self {
        Self::new_with_retry(RetryConfig::default())
    }

    pub fn new_with_retry(retry_config: RetryConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            retry_config,
        }
    }

    pub async fn download<P: AsRef<Path>>(
        &self,
        source: &VideoSource,
        output: P,
        progress_callback: Option<ProgressCallback>,
        cancel: Option<Arc<tokio::sync::Notify>>,
    ) -> Result<()> {
        if is_hls_url(&source.url) {
            return download_hls(
                &source.url,
                output.as_ref(),
                progress_callback.as_ref(),
                cancel,
            )
            .await;
        }

        let mut attempts = 0;
        let mut delay = self.retry_config.initial_delay_ms;

        loop {
            match self
                .download_impl(source, output.as_ref(), progress_callback.as_ref())
                .await
            {
                Ok(_) => return Ok(()),
                Err(e) if attempts >= self.retry_config.max_retries => {
                    return Err(e);
                }
                Err(e) => {
                    attempts += 1;
                    eprintln!(
                        "Tentative de téléchargement {}/{} échouée: {}. Nouvelle tentative dans {}ms...",
                        attempts,
                        self.retry_config.max_retries + 1,
                        e,
                        delay
                    );

                    tokio::time::sleep(Duration::from_millis(delay)).await;

                    delay = (delay as f32 * self.retry_config.backoff_multiplier) as u64;
                    delay = delay.min(self.retry_config.max_delay_ms);
                }
            }
        }
    }

    async fn download_impl<P: AsRef<Path>>(
        &self,
        source: &VideoSource,
        output: P,
        progress_callback: Option<&ProgressCallback>,
    ) -> Result<()> {
        let mut request = self.client.get(&source.url);

        if source.provider == VideoProvider::Sibnet {
            request = request.header("Referer", "https://video.sibnet.ru/");
        }

        let resp = request
            .send()
            .await
            .map_err(|e| ScraperError::NetworkError(e.to_string()))?
            .error_for_status()
            .map_err(|e| ScraperError::NetworkError(e.to_string()))?;

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        if content_type.contains("text/html")
            || content_type.contains("application/json")
            || content_type.contains("text/plain")
            || content_type.contains("application/xml")
        {
            return Err(ScraperError::NetworkError(format!(
                "réponse non-vidéo (Content-Type: {})",
                content_type
            )));
        }

        let total_size = resp.content_length().unwrap_or(0);

        let pb = if progress_callback.is_none() && total_size > 0 {
            Some(create_progress_bar(total_size))
        } else {
            None
        };

        let mut file = tokio::fs::File::create(output.as_ref())
            .await
            .map_err(|e| ScraperError::IoError(e.to_string()))?;

        let mut stream = resp.bytes_stream();
        let mut downloaded: u64 = 0;
        let start_time = std::time::Instant::now();
        let mut last_update = start_time;

        while let Some(chunk) = stream.next().await {
            let data = chunk.map_err(|e| ScraperError::NetworkError(e.to_string()))?;
            file.write_all(&data)
                .await
                .map_err(|e| ScraperError::IoError(e.to_string()))?;

            downloaded += data.len() as u64;

            if let Some(ref pb) = pb {
                pb.inc(data.len() as u64);
            }

            let now = std::time::Instant::now();
            if let Some(callback) = progress_callback {
                if now.duration_since(last_update).as_millis() >= 250
                    || (total_size > 0 && downloaded >= total_size)
                {
                    let elapsed_secs = start_time.elapsed().as_secs_f64();
                    let speed = if elapsed_secs > 0.0 {
                        (downloaded as f64 / elapsed_secs) as u64
                    } else {
                        0
                    };

                    let eta = if speed > 0 && total_size > downloaded {
                        total_size.saturating_sub(downloaded) / speed
                    } else {
                        0
                    };

                    callback(DownloadProgress {
                        id: String::new(),
                        downloaded,
                        total: total_size,
                        percentage: if total_size > 0 {
                            (downloaded as f32 / total_size as f32) * 100.0
                        } else {
                            0.0
                        },
                        speed_bytes_per_sec: speed,
                        eta_seconds: eta,
                        resolution: None,
                    });

                    last_update = now;
                }
            }
        }

        file.flush()
            .await
            .map_err(|e| ScraperError::IoError(e.to_string()))?;
        drop(file);

        if let Some(pb) = pb {
            pb.finish_with_message("Téléchargement terminé");
        }

        let written = tokio::fs::metadata(output.as_ref())
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        if written < MIN_VALID_DOWNLOAD_BYTES {
            let _ = tokio::fs::remove_file(output.as_ref()).await;
            return Err(ScraperError::NetworkError(format!(
                "fichier trop petit ({} octets), source probablement invalide",
                written
            )));
        }

        Ok(())
    }
}

impl Default for VideoDownloader {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize_scheme(url: &str) -> String {
    let u = url.trim();
    if u.starts_with("//") {
        format!("https:{}", u)
    } else {
        u.to_string()
    }
}

fn is_hls_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    let path = lower.split('?').next().unwrap_or(&lower);
    path.ends_with(".m3u8") || path.ends_with(".m3u")
}

static FFMPEG_TIME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"time=(\d+):(\d+):(\d+)\.(\d+)")
        .expect("valid ffmpeg time regex")
});
static FFMPEG_SPEED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"speed=\s*([0-9.]+)x").expect("valid ffmpeg speed regex")
});

#[derive(Debug, Clone)]
struct HlsVariant {
    url: String,
    bandwidth: u64,
    resolution: Option<String>,
}

fn join_url(base: &str, rel: &str) -> String {
    if rel.starts_with("http://") || rel.starts_with("https://") {
        return rel.to_string();
    }
    if let Some(scheme_end) = base.find("://") {
        if let Some(host_end) = base[scheme_end + 3..].find('/') {
            let scheme_host = &base[..scheme_end + 3 + host_end];
            if rel.starts_with('/') {
                return format!("{}{}", scheme_host, rel);
            }
            let dir_end = base.rfind('/').unwrap_or(base.len());
            return format!("{}/{}", &base[..dir_end], rel);
        }
    }
    rel.to_string()
}

fn parse_master_m3u8(content: &str, base_url: &str) -> Vec<HlsVariant> {
    let mut out = Vec::new();
    let mut pending: Option<(u64, Option<String>)> = None;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("#EXT-X-STREAM-INF:") {
            let attrs = &line["#EXT-X-STREAM-INF:".len()..];
            let mut bandwidth: u64 = 0;
            let mut resolution: Option<String> = None;
            for part in split_csv_attrs(attrs) {
                if let Some(rest) = part.strip_prefix("BANDWIDTH=") {
                    bandwidth = rest.trim_matches('"').parse().unwrap_or(0);
                } else if let Some(rest) = part.strip_prefix("RESOLUTION=") {
                    resolution = Some(rest.trim_matches('"').to_string());
                }
            }
            pending = Some((bandwidth, resolution));
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some((bw, res)) = pending.take() {
                out.push(HlsVariant {
                    url: join_url(base_url, line),
                    bandwidth: bw,
                    resolution: res,
                });
            }
        }
    }
    out.sort_by(|a, b| b.bandwidth.cmp(&a.bandwidth));
    out
}

fn split_csv_attrs(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                current.push(c);
            }
            ',' if !in_quotes => {
                out.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

async fn fetch_master_m3u8(url: &str) -> Result<String> {
    let (referer, origin) = pick_hls_headers(url);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| ScraperError::NetworkError(e.to_string()))?;
    let resp = client
        .get(url)
        .header("Referer", referer)
        .header("Origin", origin)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36",
        )
        .send()
        .await
        .map_err(|e| ScraperError::NetworkError(e.to_string()))?;
    let text = resp
        .text()
        .await
        .map_err(|e| ScraperError::NetworkError(e.to_string()))?;
    Ok(text)
}

async fn download_hls(
    url: &str,
    output: &Path,
    progress_callback: Option<&ProgressCallback>,
    cancel: Option<Arc<tokio::sync::Notify>>,
) -> Result<()> {
    crate::applog::log_event(
        crate::applog::LogSource::App,
        crate::applog::LogLevel::Info,
        format!("HLS détecté, fetch playlist: {}", url),
    );

    let mut variants: Vec<HlsVariant> = match fetch_master_m3u8(url).await {
        Ok(content) => parse_master_m3u8(&content, url),
        Err(e) => {
            crate::applog::log_event(
                crate::applog::LogSource::App,
                crate::applog::LogLevel::Warn,
                format!("Fetch master m3u8 KO ({}), tentative directe", e),
            );
            Vec::new()
        }
    };

    if variants.is_empty() {
        variants.push(HlsVariant {
            url: url.to_string(),
            bandwidth: 0,
            resolution: None,
        });
    } else {
        crate::applog::log_event(
            crate::applog::LogSource::App,
            crate::applog::LogLevel::Info,
            format!(
                "{} variant(s) trouvé(s) : {}",
                variants.len(),
                variants
                    .iter()
                    .map(|v| format!(
                        "{} @ {}kbps",
                        v.resolution.clone().unwrap_or_else(|| "?".to_string()),
                        v.bandwidth / 1000
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }

    let mut last_err: Option<ScraperError> = None;
    for (idx, variant) in variants.iter().enumerate() {
        crate::applog::log_event(
            crate::applog::LogSource::App,
            crate::applog::LogLevel::Info,
            format!(
                "Tentative variant {}/{} : {} ({}kbps)",
                idx + 1,
                variants.len(),
                variant.resolution.clone().unwrap_or_else(|| "?".to_string()),
                variant.bandwidth / 1000
            ),
        );
        match run_ffmpeg_hls(
            &variant.url,
            variant.resolution.clone(),
            output,
            progress_callback,
            cancel.clone(),
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(e) => {
                crate::applog::log_event(
                    crate::applog::LogSource::App,
                    crate::applog::LogLevel::Warn,
                    format!(
                        "Variant {} ({}) a échoué: {}",
                        idx + 1,
                        variant
                            .resolution
                            .clone()
                            .unwrap_or_else(|| "?".to_string()),
                        e
                    ),
                );
                last_err = Some(e);
                continue;
            }
        }
    }

    Err(last_err.unwrap_or_else(|| {
        ScraperError::NetworkError("Aucun variant HLS n'a abouti".into())
    }))
}

fn pick_hls_headers(url: &str) -> (&'static str, &'static str) {
    let lower = url.to_lowercase();
    if lower.contains("mediacache.cc") || lower.contains("uniquestream") {
        (
            "https://anime.uniquestream.net/",
            "https://anime.uniquestream.net",
        )
    } else if lower.contains("voir-anime") {
        ("https://voir-anime.to/", "https://voir-anime.to")
    } else if lower.contains("vidmoly") || lower.contains("vmeas") || lower.contains("vmwesa") {
        ("https://vidmoly.biz/", "https://vidmoly.biz")
    } else if lower.contains("sibnet") {
        ("https://video.sibnet.ru/", "https://video.sibnet.ru")
    } else if lower.contains("anikuro") {
        ("https://anikuro.to/", "https://anikuro.to")
    } else if lower.contains("mail.ru") {
        ("https://my.mail.ru/", "https://my.mail.ru")
    } else if lower.contains("voe") || lower.contains("delivery-node") {
        ("https://voe.sx/", "https://voe.sx")
    } else if lower.contains("streamtape") || lower.contains("streamta.pe") || lower.contains("tapecontent") {
        ("https://streamtape.com/", "https://streamtape.com")
    } else {
        ("https://franime.fr/", "https://franime.fr")
    }
}

async fn run_ffmpeg_hls(
    url: &str,
    resolution: Option<String>,
    output: &Path,
    progress_callback: Option<&ProgressCallback>,
    cancel: Option<Arc<tokio::sync::Notify>>,
) -> Result<()> {
    let (referer, origin) = pick_hls_headers(url);
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("info")
        .arg("-stats")
        .arg("-headers")
        .arg(format!("Referer: {}\r\nOrigin: {}\r\n", referer, origin))
        .arg("-i")
        .arg(url)
        .arg("-c")
        .arg("copy")
        .arg("-bsf:a")
        .arg("aac_adtstoasc")
        .arg(output)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| ScraperError::IoError(format!("ffmpeg introuvable: {}", e)))?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ScraperError::IoError("ffmpeg stderr pipe absent".into()))?;

    let start = std::time::Instant::now();
    let mut reader = tokio::io::BufReader::new(stderr).lines();
    let mut last_emit = std::time::Instant::now();
    let mut was_cancelled = false;
    loop {
        let line_fut = reader.next_line();
        tokio::pin!(line_fut);
        let cancelled = match &cancel {
            Some(n) => {
                let notified = n.notified();
                tokio::pin!(notified);
                tokio::select! {
                    line_result = &mut line_fut => Some(line_result),
                    _ = &mut notified => None,
                }
            }
            None => Some(line_fut.await),
        };
        let line_result = match cancelled {
            Some(r) => r,
            None => {
                was_cancelled = true;
                let _ = child.kill().await;
                crate::applog::log_event(
                    crate::applog::LogSource::App,
                    crate::applog::LogLevel::Warn,
                    "ffmpeg killé par signal de cancel",
                );
                break;
            }
        };
        let line = match line_result {
            Ok(Some(l)) => l,
            _ => break,
        };
        if let Some(cb) = progress_callback {
            if last_emit.elapsed().as_millis() >= 400 {
                let seconds_processed = FFMPEG_TIME_RE.captures(&line).map(|c| {
                    let h: u64 = c.get(1).unwrap().as_str().parse().unwrap_or(0);
                    let m: u64 = c.get(2).unwrap().as_str().parse().unwrap_or(0);
                    let s: u64 = c.get(3).unwrap().as_str().parse().unwrap_or(0);
                    h * 3600 + m * 60 + s
                });
                if let Some(secs) = seconds_processed {
                    let speed = FFMPEG_SPEED_RE
                        .captures(&line)
                        .and_then(|c| c.get(1).map(|m| m.as_str().parse::<f64>().unwrap_or(1.0)))
                        .unwrap_or(1.0);
                    let progress = DownloadProgress {
                        id: String::new(),
                        downloaded: secs,
                        total: 0,
                        percentage: 0.0,
                        speed_bytes_per_sec: (speed * 1024.0 * 1024.0) as u64,
                        eta_seconds: 0,
                        resolution: resolution.clone(),
                    };
                    cb(progress);
                    last_emit = std::time::Instant::now();
                }
            }
        }
        if line.to_lowercase().contains("error")
            || line.to_lowercase().contains("invalid data")
        {
            crate::applog::log_event(
                crate::applog::LogSource::App,
                crate::applog::LogLevel::Warn,
                format!("ffmpeg: {}", line),
            );
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| ScraperError::IoError(format!("ffmpeg wait: {}", e)))?;
    if was_cancelled {
        return Err(ScraperError::NetworkError("Cancelled".into()));
    }
    if !status.success() {
        return Err(ScraperError::NetworkError(format!(
            "ffmpeg a échoué (code {:?}) après {:.1}s",
            status.code(),
            start.elapsed().as_secs_f32()
        )));
    }
    let written = tokio::fs::metadata(output)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if written < MIN_VALID_DOWNLOAD_BYTES {
        let _ = tokio::fs::remove_file(output).await;
        return Err(ScraperError::NetworkError(format!(
            "HLS: fichier de sortie trop petit ({} octets), flux probablement invalide",
            written
        )));
    }
    crate::applog::log_event(
        crate::applog::LogSource::App,
        crate::applog::LogLevel::Info,
        format!(
            "HLS terminé en {:.1}s ({}): {}",
            start.elapsed().as_secs_f32(),
            resolution.clone().unwrap_or_else(|| "?".to_string()),
            output.display()
        ),
    );
    Ok(())
}

fn create_progress_bar(total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
        )
        .unwrap()
        .progress_chars("=> "),
    );
    pb
}
