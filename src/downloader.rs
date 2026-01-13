use chromiumoxide::cdp::browser_protocol::network::{
    CookieParam, CookiePartitionKey, CookieSameSite, EnableParams, EventResponseReceived,
    TimeSinceEpoch,
};
use chromiumoxide::cdp::browser_protocol::page::NavigateParams;
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, RwLock};

#[derive(Debug, Clone)]
pub enum ScraperError {
    BrowserLaunch(String),
    Navigation(String),
    IframeNotFound,
    VideoSourceNotFound,
    Timeout(String),
    UnsupportedProvider(String),
    DownloadFailed(String),
    IoError(String),
    NetworkError(String),
}

impl std::fmt::Display for ScraperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BrowserLaunch(msg) => write!(f, "Erreur de lancement du navigateur: {}", msg),
            Self::Navigation(msg) => write!(f, "Erreur de navigation: {}", msg),
            Self::IframeNotFound => write!(f, "Iframe non trouvée"),
            Self::VideoSourceNotFound => write!(f, "Source vidéo non trouvée"),
            Self::Timeout(msg) => write!(f, "Timeout: {}", msg),
            Self::UnsupportedProvider(url) => write!(f, "Provider non supporté: {}", url),
            Self::DownloadFailed(msg) => write!(f, "Échec du téléchargement: {}", msg),
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
    Unknown,
}

#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub id: String,
    pub downloaded: u64,
    pub total: u64,
    pub percentage: f32,
    pub speed_bytes_per_sec: u64,
    pub eta_seconds: u64,
}

#[derive(Debug, Clone)]
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
            max_retries: 1,
            initial_delay_ms: 1000,
            max_delay_ms: 10000,
            backoff_multiplier: 2.0,
        }
    }
}

pub struct FranimeScraper {
    browser: Browser,
    retry_config: RetryConfig,
}

impl FranimeScraper {
    pub async fn new(headless: bool) -> Result<Self> {
        Self::new_with_retry(headless, RetryConfig::default()).await
    }

    pub async fn new_with_retry(headless: bool, retry_config: RetryConfig) -> Result<Self> {
        let config = BrowserConfig::builder();
        let config = if headless {
            config
                .build()
                .map_err(|e| ScraperError::BrowserLaunch(e.to_string()))?
        } else {
            config
                .with_head()
                .build()
                .map_err(|e| ScraperError::BrowserLaunch(e.to_string()))?
        };

        let (browser, mut handler) = Browser::launch(config)
            .await
            .map_err(|e| ScraperError::BrowserLaunch(e.to_string()))?;

        tokio::spawn(async move { while handler.next().await.is_some() {} });

        Ok(Self {
            browser,
            retry_config,
        })
    }

    async fn setup_cloudflare_cookie(&self, _page: &Page) -> Result<()> {
        let cookie = CookieParam::builder()
            .expires(TimeSinceEpoch::new(1799665392.0))
            .secure(true)
            .http_only(true)
            .same_site(CookieSameSite::None)
            .domain(".franime.fr")
            .path("/")
            .name("cf_clearance")
            .value("_8bup68CEChcEz15Q4QY65VzBn5BFopxSTybAdL1lvU-1768283737-1.2.1.1-K1OeiOg7kGPPq5l26LnXuYMQdiUIaBbetyaX8caqPX014WS5rierp_LXFgzWtR1AJoQPLwdmDmnsniIfOreq2Y4LiGCddOacIB7fOBpLj4snKDRlQDBIkYHHBDxUNbK2Vr4ZZxi7hCboXfDUnI7Iv3s5zJf5FXcGL.xzgqX5VALDOSLoeB0XbzUaUrBauvst_Fy6Ho6ReZJhsrqyr6IS6ucLuYZIQ2o4W419ymmHrH0UCqSBOr9LNkjcTC1aU8A7")
            .partition_key(CookiePartitionKey::new("https://franime.fr", false))
            .build()
            .map_err(|e| ScraperError::BrowserLaunch(e.to_string()))?;

        self.browser
            .set_cookies(vec![cookie])
            .await
            .map_err(|e| ScraperError::BrowserLaunch(e.to_string()))?;

        Ok(())
    }

    pub async fn extract_video_source(&self, url: &str) -> Result<VideoSource> {
        self.retry_operation(|| self.extract_video_source_impl(url))
            .await
    }

    async fn extract_video_source_impl(&self, url: &str) -> Result<VideoSource> {
        let page = self
            .browser
            .new_page("https://franime.fr")
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;

        self.setup_cloudflare_cookie(&page).await?;

        let page = page
            .goto(
                NavigateParams::builder()
                    .url(url)
                    .build()
                    .map_err(|e| ScraperError::Navigation(e.to_string()))?,
            )
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?
            .wait_for_navigation()
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;

        self.wait_for_iframes(&page, 2).await?;

        let iframe_src = self.extract_iframe_src(&page).await?;

        let provider = if iframe_src.contains("sibnet.ru") {
            VideoProvider::Sibnet
        } else if iframe_src.contains("sendvid.com") {
            VideoProvider::Sendvid
        } else if iframe_src.contains("filemoon.to") {
            println!("FileMoon detected");
            VideoProvider::FileMoon
        } else {
            VideoProvider::Unknown
        };

        let video_url = match provider {
            VideoProvider::Sibnet => self.extract_sibnet_url(&page, &iframe_src).await?,
            VideoProvider::Sendvid => self.extract_sendvid_url(&page, &iframe_src).await?,
            VideoProvider::FileMoon => self.extract_filemoon_url(&page, &iframe_src).await?,
            VideoProvider::Unknown => {
                return Err(ScraperError::UnsupportedProvider(iframe_src));
            }
        };

        Ok(VideoSource {
            url: video_url,
            provider,
        })
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

    async fn wait_for_iframes(&self, page: &Page, count: usize) -> Result<()> {
        let timeout = tokio::time::Duration::from_secs(10);
        let start = tokio::time::Instant::now();

        loop {
            if start.elapsed() > timeout {
                return Err(ScraperError::Timeout("Attente des iframes".to_string()));
            }

            let frames = page
                .frames()
                .await
                .map_err(|e| ScraperError::Navigation(e.to_string()))?;
            if frames.len() >= count {
                break;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        Ok(())
    }

    async fn extract_iframe_src(&self, page: &Page) -> Result<String> {
        let iframes = page
            .find_xpaths("/html/body/iframe")
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;

        let iframe = iframes.last().ok_or(ScraperError::IframeNotFound)?;

        iframe
            .attribute("src")
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?
            .ok_or(ScraperError::IframeNotFound)
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

        video_page
            .evaluate(
                r#"
                () => {
                    const v = document.querySelector('video');
                    if (!v) return "NO_VIDEO";
                    v.muted = true;
                    v.play();
                    return "PLAYING";
                }
                "#,
            )
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;

        let timeout = tokio::time::Duration::from_secs(15);
        let start = tokio::time::Instant::now();

        while let Some(ev) = net_events.next().await {
            if start.elapsed() > timeout {
                return Err(ScraperError::Timeout("Attente de l'URL vidéo".to_string()));
            }

            let res = &ev.response;

            if res.mime_type == "video/mp4" && res.url.contains("sibnet.ru") {
                video_page
                    .evaluate(
                        r#"
                        () => {
                            const v = document.querySelector('video');
                            if (v) v.pause();
                        }
                        "#,
                    )
                    .await
                    .map_err(|e| ScraperError::Navigation(e.to_string()))?;

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

        let video_elem = video_page
            .find_xpath(r#"//*[@id="video_source"]"#)
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;

        let src = video_elem
            .attribute("src")
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?
            .ok_or(ScraperError::VideoSourceNotFound)?;

        if src == "undefined" {
            return Err(ScraperError::VideoSourceNotFound);
        }

        Ok(src)
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
        println!("Attente de l'URL vidéo...");
        while let Some(ev) = net_events.next().await {
            if start.elapsed() > timeout {
                return Err(ScraperError::Timeout("Attente de l'URL vidéo".to_string()));
            }

            let res = &ev.response;

            println!("URL: {} {}", res.url, res.mime_type);

            if res.mime_type == "" && res.url.contains("filemoon.to") {
                return Ok(res.url.clone());
            }
        }

        Err(ScraperError::VideoSourceNotFound)
    }
}

pub struct DownloadManager {
    scraper: Arc<FranimeScraper>,
    downloader: Arc<VideoDownloader>,
    tasks: Arc<RwLock<Vec<DownloadTask>>>,
    max_concurrent: usize,
    update_tx: mpsc::UnboundedSender<DownloadTask>,
}

impl DownloadManager {
    pub async fn new(
        headless: bool,
        max_concurrent: usize,
    ) -> Result<(Self, mpsc::UnboundedReceiver<DownloadTask>)> {
        let scraper = Arc::new(FranimeScraper::new(headless).await?);
        let downloader = Arc::new(VideoDownloader::new());
        let tasks = Arc::new(RwLock::new(Vec::new()));
        let (update_tx, update_rx) = mpsc::unbounded_channel();

        Ok((
            Self {
                scraper,
                downloader,
                tasks,
                max_concurrent,
                update_tx,
            },
            update_rx,
        ))
    }

    pub async fn add_download(
        &self,
        url: String,
        output_path: String,
        output_name: String,
    ) -> String {
        fs::create_dir_all(&output_path).unwrap();
        let output_path = Path::new(&output_path).join(output_name);
        let id = uuid::Uuid::new_v4().to_string();
        let task = DownloadTask {
            id: id.clone(),
            url,
            output_path,
            status: DownloadStatus::Queued,
        };

        self.tasks.write().await.push(task.clone());
        self.notify_update(task);

        let manager = self.clone_for_task();
        let id_clone = id.clone();
        tokio::spawn(async move {
            manager.process_task(id_clone).await;
        });

        id
    }

    pub async fn cancel_download(&self, id: &str) -> Result<()> {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.iter_mut().find(|t| t.id == id) {
            task.status = DownloadStatus::Cancelled;
            self.notify_update(task.clone());
            Ok(())
        } else {
            Err(ScraperError::DownloadFailed(format!(
                "Tâche non trouvée: {}",
                id
            )))
        }
    }

    pub async fn get_tasks(&self) -> Vec<DownloadTask> {
        self.tasks.read().await.clone()
    }

    pub async fn get_task(&self, id: &str) -> Option<DownloadTask> {
        self.tasks.read().await.iter().find(|t| t.id == id).cloned()
    }

    async fn process_task(&self, id: String) {
        loop {
            let active_count = self
                .tasks
                .read()
                .await
                .iter()
                .filter(|t| {
                    matches!(
                        t.status,
                        DownloadStatus::Extracting | DownloadStatus::Downloading(_)
                    )
                })
                .count();

            if active_count < self.max_concurrent {
                break;
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        let task = {
            let tasks = self.tasks.read().await;
            tasks.iter().find(|t| t.id == id).cloned()
        };

        let Some(task) = task else {
            return;
        };

        if matches!(task.status, DownloadStatus::Cancelled) {
            return;
        }

        self.update_task_status(&id, DownloadStatus::Extracting)
            .await;

        let source = match self.scraper.extract_video_source(&task.url).await {
            Ok(s) => s,
            Err(e) => {
                let error_msg = e.to_string();
                self.update_task_status(&id, DownloadStatus::Failed(error_msg))
                    .await;
                return;
            }
        };

        if self.is_cancelled(&id).await {
            return;
        }

        let id_clone = id.clone();
        let update_tx = self.update_tx.clone();
        let tasks = self.tasks.clone();
        let output = task.output_path.clone();

        let result = self
            .downloader
            .download(
                &source,
                &output.clone(),
                Some(Box::new(move |progress| {
                    let progress_clone = progress.clone();

                    let task = DownloadTask {
                        id: id_clone.clone(),
                        url: String::new(),
                        output_path: output.clone(),
                        status: DownloadStatus::Downloading(progress_clone.clone()),
                    };

                    let _ = update_tx.send(task);

                    let tasks = tasks.clone();
                    let id = id_clone.clone();
                    tokio::spawn(async move {
                        let mut tasks = tasks.write().await;
                        if let Some(t) = tasks.iter_mut().find(|t| t.id == id) {
                            t.status = DownloadStatus::Downloading(progress_clone);
                        }
                    });
                })),
            )
            .await;

        match result {
            Ok(_) if !self.is_cancelled(&id).await => {
                self.update_task_status(&id, DownloadStatus::Completed)
                    .await;
            }
            Err(e) if !self.is_cancelled(&id).await => {
                let error_msg = e.to_string();
                self.update_task_status(&id, DownloadStatus::Failed(error_msg))
                    .await;
            }
            _ => {}
        }
    }

    async fn update_task_status(&self, id: &str, status: DownloadStatus) {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.iter_mut().find(|t| t.id == id) {
            task.status = status;
            self.notify_update(task.clone());
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

    fn notify_update(&self, task: DownloadTask) {
        let _ = self.update_tx.send(task);
    }

    fn clone_for_task(&self) -> Self {
        Self {
            scraper: self.scraper.clone(),
            downloader: self.downloader.clone(),
            tasks: self.tasks.clone(),
            max_concurrent: self.max_concurrent,
            update_tx: self.update_tx.clone(),
        }
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
    ) -> Result<()> {
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
            if let Some(ref callback) = progress_callback {
                if now.duration_since(last_update).as_millis() >= 100 || downloaded == total_size {
                    let elapsed_secs = start_time.elapsed().as_secs_f64();
                    let speed = if elapsed_secs > 0.0 {
                        (downloaded as f64 / elapsed_secs) as u64
                    } else {
                        0
                    };

                    let eta = if speed > 0 && total_size > downloaded {
                        ((total_size - downloaded) / speed)
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
                    });

                    last_update = now;
                }
            }
        }

        if let Some(pb) = pb {
            pb.finish_with_message("Téléchargement terminé");
        }

        Ok(())
    }

    pub async fn download_simple<P: AsRef<Path>>(
        &self,
        source: &VideoSource,
        output: P,
    ) -> Result<()> {
        self.download(source, output, None).await
    }
}

impl Default for VideoDownloader {
    fn default() -> Self {
        Self::new()
    }
}

fn create_progress_bar(total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})"
        )
            .unwrap()
            .progress_chars("=> "),
    );
    pb
}

#[tokio::main]
async fn main() -> Result<()> {
    simple_download_example().await?;

    Ok(())
}

async fn simple_download_example() -> Result<()> {
    let url = "https://franime.fr/watch2/?a=NjgzODNmM2EzYjY3NmM2NjY5M2MzZDNiM2E2NzNhMzgzYTNkNjk2YTZlM2I2ZTZkNjgzYjY5NmYzODM4NmI2ZDM4NmE2ZTY2NmI2ZTZiNmY2OTY5NmQ2ZDY2M2YzZjY5M2I2ZjNjM2E2NjNhNmY2ODNkNmM2YjY5NjYzYjZjM2YzYTY5NmIzYQ%3D%3D&b=MzYyYTJhMmUyZDY0NzE3MTM4MzczMjNiMzMzMTMxMzA3MDJhMzE3MTNiNzEzMDZmMzEzMzJkNjk2YjMzMzQzYzI0MmI%3D&c=NjY2ZjY5M2I2ZjZjM2I2ZTNhNmM2ZDY4NmE2OTZiM2QzODZhNmYzZDZjNmQ2OTZiM2M2YjM4NmY2YzZhMzg2YjNiM2I2ZDZhMzg2YzNjM2E2YTZlM2M2ODNkNmY2NzM4M2YzODM4M2I2YjNmNmQ2YTNmNjgzYjNiNjg2YTZiM2M2YTM4MzgzZA%3D%3D&d=MDRiNGFhZThlMTQ3MTY2MDJlN2M1OTQxNDMwMjU3MWFkN2M3ZDg4ZmFiNmVmOWQ5NTYxYjYzMzJhOTgyM2JjMjlhZGMxMzE0OTliYjMwZDkwODM3ZTdiYQ%3D%3D&e=Njk2YjNiM2MzYjY2NjgzYzY2NmYzYTZkNjg2ZTY4NmYzYzY4M2I2YjNmM2Q2ZjZmM2Y2OTNmM2Y2ODNkNmY2YjZiM2Y2NjNiNjg2ZjY3NmM2NjNmNmQ2ZDY5NjczYTNhM2M2YTY2NmQ2NzZlM2EzYTZhMzgzZjNm&f=Njk2YzY3NmU2YzZlM2EzYTZhNjczYjNiM2E2YTNjNjYzZjY4NmQzYzZlNmIzODNjM2M2YzNkNmE2ZjNmNmMzYzNkNmYzYTZmNmUzODY2NmY2OTZjMzgzYjZkNjY2ZjZjNjY2ZjZiNjYzYTZhM2MzZjNhNmI2ODY4NmI2YzY2NmUzYTZiNmM2NjNhM2QzZDNkM2EzYTZmNmY2YjZlNmQzYjNmNmQ2YjY4Mzg2ZDNmNmQ2ZTNiM2YzZjY2M2QzYjM4Njg2YjNkNmEzYTY3NmEzYjY2NmE2ZDY4&g=NmE2YzZlM2Q2ZDZlNmEzZDZiM2EzYzY2NmQ2YjNiM2Y2ZjNhNmYzZjZjNmQ2NzZkNjgzYzNkNmQ2NjNiMzgzYTY5NmE2ODNjNjY2OTNiNmUzZjY2NjY2ZDZhMzg2YzY2M2M2ODY3NmI2OTNmNjY2YTZkMzgzYTZjM2Q2NzZhNjc2YTY5NmYzYjZmM2E2ODY2MzgzZjZjM2IzZDNiMzgzYjM4Njk2ZjZhNjk2ZjY3M2Q2ZjZkMzg2ZTY2M2E%3D&h=NjgzYjZmM2M2NjZiNjkzZjY5M2I2YjNmMzgzZDNkNmY2ZjZhNjc2ODNjNjkzZjY4Mzg2ZTZjNjc2ZDZlNmE2NzNjNmE2ZDNjM2MzYTNiNmEzZjZiNjg2NzNmNjgzYTY3NmI2NzNmM2MzYTY4NmU2YjZmNjk2ZTY3M2I2OQ%3D%3D&i=ZDdlNWU4MDFkOGE3OWI4ZGI0ODY0ZDk2Zjc0ZjlhZmQwMDhlMmFiZWIxNGUxMDg4YmMyNDgxOTI5YTFhODU4MTJmOTkyZjRjYzljNzZlMmUzYTc4ZjJlMDhkYTM%3D&j=ZmIyZDViYzc1MGMyZjU5OTA0NGJiYTZjNTk4NDBjNGM4Mzk4NGUzMDJmOWM1MmE0NWFlYjM3NmNhYmI5MTk5ZTUwYzc4N2RhZjBlNw%3D%3D&k=NjkzZDZhM2IzYjNkM2QzYjNkNjczYjZmNmI2YTY5NmI2YjZiNmQ2ODY3NjgzZjZmM2MzYTZkNmM2YzY2Mzg2ZTZjNmM2ZTNmM2QzZjZkM2I2NjZkNjg2ZDZmMzgzODZhNmM2ZDY5M2E2NzZjNmI2YTY3M2QzODNiNmI2ZDM4NmIzYjZlNmQ2YzNjM2I2ZTZiNmMzZDNhNjc2ZjY2NmIzYzNmM2QzYTY5Mzg2ZA%3D%3D&l=M2IzYzY5NmIzYTZlM2I2ZDNmNmY2YzY4M2IzODZiM2QzYjY5NjgzZjNjNjg2NjZkM2E2YjZhNjc2OTM4M2Q2OTNjM2I2ZjNjNjczZDZlM2Y2ZTZlM2E2YTNiNmM2ZjZmNmU2NjZjM2I2ODY3NjgzZjY3NmE2NzNmNjk2NzZjNjYzZDZiMzgzYTNkNmI2YTM4M2E2ZTNhM2M2ZDY2M2Q2ODM4NmQzZDY4NmMzZjY5NmIzYTNkNmMzZjM4M2IzYzNj&m=OWZjMTAxNjBjY2FjMjQ1ZWQ0MDBiNTA0ZTRhMGZkNDE3MWE1ZjQ0NzM4ZmNlOTdjNTJjMzhkN2Q2MDZhODI0MGI1OGRhMjdhN2E1&n=NmIzZjNmNjg2YTZiNjczODZmNjczYzM4NmM2YTZkM2EzZDZjM2Y2ZTNmM2M2OTY5NmUzYjY4Njg2YjZiNmEzYzZjMzg2NzZkM2M2OTZiNmUzYTNjM2Y2NjZiNjczZDZhNjg2ZDZmNmUzYzNhNmY2ZjZmNmIzYzNhNmUzODM4NjkzZjZlNjc2ZDM4M2Y2ODZjNmEzZjZlNjY2YjNiNjk2YjNhM2YzYTY5NmMzZjY2Njg2NjNiNmE2NjY2M2IzODZlNmI2ZjY3NmE2ZjNmM2Y2ODZhNjgzZDNmNjg2ZTZmNmU2ZTNk&o=OWRmMWEzMTdhYjdkYTEyMWRhYzZjMzdlOGYyYWJhNTUyZGZhOWU0NzI0NTU5NTg2Y2ViYWNkZjcxYWE1OWUwNWIzMjg2YWE5";

    let scraper = FranimeScraper::new(false).await?;

    println!("Extraction de la source vidéo...");
    let source = scraper.extract_video_source(url).await?;
    println!("Source trouvée: {:?}", source.provider);
    println!("URL: {}", source.url);

    let downloader = VideoDownloader::new();
    println!("Téléchargement en cours...");
    downloader.download_simple(&source, "video.mp4").await?;
    println!("Téléchargement terminé!");

    Ok(())
}

#[allow(dead_code)]
async fn multi_download_example() -> Result<()> {
    let (manager, mut updates) = DownloadManager::new(true, 3).await?;

    let urls = vec![
        ("https://franime.fr/watch2/?a=...", "video1.mp4"),
        ("https://franime.fr/watch2/?b=...", "video2.mp4"),
        ("https://franime.fr/watch2/?c=...", "video3.mp4"),
    ];

    for (url, output) in urls {
        let id = manager
            .add_download(url.to_string(), output.to_string(), "".to_string())
            .await;
        println!("Tâche ajoutée: {}", id);
    }

    tokio::spawn(async move {
        while let Some(task) = updates.recv().await {
            match task.status {
                DownloadStatus::Queued => {
                    println!("[{}] En attente...", task.id);
                }
                DownloadStatus::Extracting => {
                    println!("[{}] Extraction de la source...", task.id);
                }
                DownloadStatus::Downloading(ref progress) => {
                    println!(
                        "[{}] Téléchargement: {:.1}% ({} MB/s, ETA: {}s)",
                        task.id,
                        progress.percentage,
                        progress.speed_bytes_per_sec / 1_000_000,
                        progress.eta_seconds
                    );
                }
                DownloadStatus::Completed => {
                    println!("[{}] ✓ Terminé!", task.id);
                }
                DownloadStatus::Failed(ref err) => {
                    eprintln!("[{}] ✗ Échec: {}", task.id, err);
                }
                DownloadStatus::Cancelled => {
                    println!("[{}] Annulé", task.id);
                }
            }
        }
    });

    loop {
        let tasks = manager.get_tasks().await;
        let all_done = tasks.iter().all(|t| {
            matches!(
                t.status,
                DownloadStatus::Completed | DownloadStatus::Failed(_) | DownloadStatus::Cancelled
            )
        });

        if all_done {
            break;
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    println!("Tous les téléchargements sont terminés!");
    Ok(())
}
