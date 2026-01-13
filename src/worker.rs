use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::runtime::Runtime;

#[derive(Debug, Clone)]
pub enum WorkerTask {
    SyncApi {
        api_url: String,
    },
    DownloadImage {
        url: String,
        anime_id: f64,
    },
    DownloadAnime {
        anime_id: f64,
        episode_urls: Vec<String>,
    },
    DownloadEpisode {
        anime_id: f64,
        season: usize,
        episode: usize,
        url: String,
    },
}

#[derive(Debug, Clone)]
pub enum WorkerResult {
    ApiSyncComplete {
        total: usize,
        saved: usize,
        failed: usize,
    },
    ImageDownloaded {
        url: String,
        anime_id: f64,
        success: bool,
    },
    AnimeDownloadProgress {
        anime_id: f64,
        current: usize,
        total: usize,
    },
    AnimeDownloadComplete {
        anime_id: f64,
        success: bool,
    },
    EpisodeDownloaded {
        anime_id: f64,
        season: usize,
        episode: usize,
        success: bool,
    },
    Error {
        task: String,
        message: String,
    },
}

pub type ResultSender = Sender<WorkerResult>;
pub type ResultReceiver = Receiver<WorkerResult>;

pub struct WorkerManager {
    task_sender: Sender<WorkerTask>,
    result_receiver: ResultReceiver,
    runtime: Arc<Runtime>,
    workers: Vec<thread::JoinHandle<()>>,
}

impl WorkerManager {
    pub fn new(num_workers: usize) -> Self {
        let (task_tx, task_rx) = channel::<WorkerTask>();
        let (result_tx, result_rx) = channel::<WorkerResult>();
        let task_rx = Arc::new(Mutex::new(task_rx));
        let runtime = Arc::new(Runtime::new().expect("Failed to create runtime"));

        let mut workers = Vec::new();

        for worker_id in 0..num_workers {
            let task_rx = Arc::clone(&task_rx);
            let result_tx = result_tx.clone();
            let runtime = Arc::clone(&runtime);

            let handle = thread::spawn(move || {
                println!("Worker {} started", worker_id);

                loop {
                    let task = {
                        let rx = task_rx.lock().unwrap();
                        rx.recv()
                    };

                    match task {
                        Ok(task) => {
                            Self::execute_task(task, &result_tx, &runtime, worker_id);
                        }
                        Err(_) => {
                            println!("Worker {} shutting down", worker_id);
                            break;
                        }
                    }
                }
            });

            workers.push(handle);
        }

        Self {
            task_sender: task_tx,
            result_receiver: result_rx,
            runtime,
            workers,
        }
    }

    fn execute_task(
        task: WorkerTask,
        result_tx: &ResultSender,
        runtime: &Runtime,
        worker_id: usize,
    ) {
        match task {
            WorkerTask::SyncApi { api_url } => {
                println!("Worker {} syncing API: {}", worker_id, api_url);
                Self::sync_api(api_url, result_tx, runtime);
            }
            WorkerTask::DownloadImage { url, anime_id } => {
                println!(
                    "Worker {} downloading image for anime {}",
                    worker_id, anime_id
                );
                Self::download_image(url, anime_id, result_tx, runtime);
            }
            WorkerTask::DownloadAnime {
                anime_id,
                episode_urls,
            } => {
                println!(
                    "Worker {} downloading anime {} ({} episodes)",
                    worker_id,
                    anime_id,
                    episode_urls.len()
                );
                Self::download_anime(anime_id, episode_urls, result_tx, runtime);
            }
            WorkerTask::DownloadEpisode {
                anime_id,
                season,
                episode,
                url,
            } => {
                println!(
                    "Worker {} downloading S{}E{} of anime {}",
                    worker_id, season, episode, anime_id
                );
                Self::download_episode(anime_id, season, episode, url, result_tx, runtime);
            }
        }
    }

    fn sync_api(api_url: String, result_tx: &ResultSender, runtime: &Runtime) {
        let result = runtime.block_on(async {
            match reqwest::get(&api_url).await {
                Ok(response) => match response.text().await {
                    Ok(json_text) => Ok(json_text),
                    Err(e) => Err(format!("Failed to read response: {}", e)),
                },
                Err(e) => Err(format!("Failed to fetch API: {}", e)),
            }
        });

        match result {
            Ok(_json) => {
                let _ = result_tx.send(WorkerResult::ApiSyncComplete {
                    total: 0,
                    saved: 0,
                    failed: 0,
                });
            }
            Err(e) => {
                let _ = result_tx.send(WorkerResult::Error {
                    task: "SyncApi".to_string(),
                    message: e,
                });
            }
        }
    }

    fn download_image(url: String, anime_id: f64, result_tx: &ResultSender, runtime: &Runtime) {
        let result = runtime.block_on(async {
            match reqwest::get(&url).await {
                Ok(response) => match response.bytes().await {
                    Ok(bytes) => Ok(bytes.to_vec()),
                    Err(e) => Err(format!("Failed to read bytes: {}", e)),
                },
                Err(e) => Err(format!("Failed to download: {}", e)),
            }
        });

        let success = result.is_ok();

        let _ = result_tx.send(WorkerResult::ImageDownloaded {
            url,
            anime_id,
            success,
        });
    }

    fn download_anime(
        anime_id: f64,
        episode_urls: Vec<String>,
        result_tx: &ResultSender,
        runtime: &Runtime,
    ) {
        let total = episode_urls.len();
        let mut downloaded = 0;

        for (idx, url) in episode_urls.iter().enumerate() {
            let result = runtime.block_on(async {
                match reqwest::get(url).await {
                    Ok(response) => match response.bytes().await {
                        Ok(_bytes) => Ok(()),
                        Err(e) => Err(format!("Failed to read bytes: {}", e)),
                    },
                    Err(e) => Err(format!("Failed to download: {}", e)),
                }
            });

            if result.is_ok() {
                downloaded += 1;
            }

            let _ = result_tx.send(WorkerResult::AnimeDownloadProgress {
                anime_id,
                current: idx + 1,
                total,
            });
        }

        let _ = result_tx.send(WorkerResult::AnimeDownloadComplete {
            anime_id,
            success: downloaded == total,
        });
    }

    fn download_episode(
        anime_id: f64,
        season: usize,
        episode: usize,
        url: String,
        result_tx: &ResultSender,
        runtime: &Runtime,
    ) {
        let result = runtime.block_on(async {
            match reqwest::get(&url).await {
                Ok(response) => match response.bytes().await {
                    Ok(bytes) => Ok(bytes.to_vec()),
                    Err(e) => Err(format!("Failed to read bytes: {}", e)),
                },
                Err(e) => Err(format!("Failed to download: {}", e)),
            }
        });

        let success = result.is_ok();

        let _ = result_tx.send(WorkerResult::EpisodeDownloaded {
            anime_id,
            season,
            episode,
            success,
        });
    }

    pub fn submit_task(&self, task: WorkerTask) {
        let _ = self.task_sender.send(task);
    }

    pub fn try_recv_result(&self) -> Option<WorkerResult> {
        self.result_receiver.try_recv().ok()
    }

    pub fn recv_result(&self) -> Option<WorkerResult> {
        self.result_receiver.recv().ok()
    }
}

pub struct WorkerPool {
    manager: Arc<Mutex<WorkerManager>>,
}

impl WorkerPool {
    pub fn new(num_workers: usize) -> Self {
        Self {
            manager: Arc::new(Mutex::new(WorkerManager::new(num_workers))),
        }
    }

    pub fn submit(&self, task: WorkerTask) {
        if let Ok(manager) = self.manager.lock() {
            manager.submit_task(task);
        }
    }

    pub fn check_results(&self) -> Vec<WorkerResult> {
        let mut results = Vec::new();

        if let Ok(manager) = self.manager.lock() {
            while let Some(result) = manager.try_recv_result() {
                results.push(result);
            }
        }

        results
    }

    pub fn sync_api(&self, api_url: String) {
        self.submit(WorkerTask::SyncApi { api_url });
    }

    pub fn download_image(&self, url: String, anime_id: f64) {
        self.submit(WorkerTask::DownloadImage { url, anime_id });
    }

    pub fn download_anime(&self, anime_id: f64, episode_urls: Vec<String>) {
        self.submit(WorkerTask::DownloadAnime {
            anime_id,
            episode_urls,
        });
    }

    pub fn download_episode(&self, anime_id: f64, season: usize, episode: usize, url: String) {
        self.submit(WorkerTask::DownloadEpisode {
            anime_id,
            season,
            episode,
            url,
        });
    }
}

impl Drop for WorkerManager {
    fn drop(&mut self) {
        drop(self.task_sender.clone());

        while let Some(handle) = self.workers.pop() {
            let _ = handle.join();
        }
    }
}
