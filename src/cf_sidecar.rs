use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, oneshot};

const RESULT_PREFIX: &str = "__FRANIME_RESULT__";

#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    #[error("Python introuvable ({0}): {1}")]
    PythonNotFound(String, std::io::Error),
    #[error("Script python introuvable: {0}")]
    ScriptNotFound(PathBuf),
    #[error("Sidecar process mort (code {0:?})")]
    ProcessDead(Option<i32>),
    #[error("Sidecar a renvoyé une erreur: {0}")]
    Remote(String),
    #[error("JSON invalide: {0}")]
    BadJson(#[from] serde_json::Error),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("Sidecar non démarré")]
    NotStarted,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Ready {
    pub user_agent: String,
    pub all_cookies: HashMap<String, String>,
}

#[derive(Debug, serde::Deserialize)]
struct Response {
    id: Option<u64>,
    iframe_src: Option<String>,
    video_url: Option<String>,
    error: Option<String>,
    #[serde(default)]
    cf_clearance: Option<String>,
    #[serde(default)]
    user_agent: Option<String>,
    #[serde(default)]
    all_cookies: Option<HashMap<String, String>>,
    #[serde(default, rename = "type")]
    msg_type: Option<String>,
    #[serde(default)]
    pong: Option<bool>,
    #[serde(default)]
    ok: Option<bool>,
}

#[derive(Debug)]
pub enum SidecarReply {
    IframeSrc(String),
    VideoUrl(String),
    Refreshed {
        user_agent: String,
        cookies: HashMap<String, String>,
    },
    Ack,
    Error(String),
}

pub struct Sidecar {
    inner: Mutex<Option<SidecarInner>>,
    next_id: AtomicU64,
    headless: AtomicBool,
    cached_ready: Mutex<Option<Ready>>,
    started_at: std::sync::atomic::AtomicI64,
    cf_solves: AtomicU64,
    fetch_ok: AtomicU64,
    fetch_err: AtomicU64,
    refresh_calls: AtomicU64,
}

#[derive(Debug, Clone, Copy)]
pub struct SidecarMetrics {
    pub started_at: i64,
    pub cf_solves: u64,
    pub fetch_ok: u64,
    pub fetch_err: u64,
    pub refresh_calls: u64,
    pub is_alive: bool,
}

struct SidecarInner {
    child: Child,
    stdin: ChildStdin,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<SidecarReply>>>>,
}

impl Sidecar {
    pub fn new(headless: bool) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(None),
            next_id: AtomicU64::new(1),
            headless: AtomicBool::new(headless),
            cached_ready: Mutex::new(None),
            started_at: std::sync::atomic::AtomicI64::new(0),
            cf_solves: AtomicU64::new(0),
            fetch_ok: AtomicU64::new(0),
            fetch_err: AtomicU64::new(0),
            refresh_calls: AtomicU64::new(0),
        })
    }

    pub fn set_headless(&self, headless: bool) {
        self.headless.store(headless, Ordering::SeqCst);
    }

    pub async fn metrics(&self) -> SidecarMetrics {
        let is_alive = self.inner.lock().await.is_some();
        SidecarMetrics {
            started_at: self.started_at.load(Ordering::SeqCst),
            cf_solves: self.cf_solves.load(Ordering::SeqCst),
            fetch_ok: self.fetch_ok.load(Ordering::SeqCst),
            fetch_err: self.fetch_err.load(Ordering::SeqCst),
            refresh_calls: self.refresh_calls.load(Ordering::SeqCst),
            is_alive,
        }
    }

    pub async fn restart(self: &Arc<Self>) {
        let mut guard = self.inner.lock().await;
        if let Some(inner) = guard.take() {
            let mut child = inner.child;
            drop(inner.stdin);
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        *self.cached_ready.lock().await = None;
        self.started_at.store(0, Ordering::SeqCst);
    }

    pub async fn is_alive(&self) -> bool {
        self.inner.lock().await.is_some()
    }

    pub async fn ensure_started(self: &Arc<Self>) -> Result<Ready, SidecarError> {
        {
            let inner_alive = self.inner.lock().await.is_some();
            if inner_alive {
                if let Some(r) = self.cached_ready.lock().await.clone() {
                    return Ok(r);
                }
            }
        }
        let mut guard = self.inner.lock().await;
        if guard.is_some() {
            if let Some(r) = self.cached_ready.lock().await.clone() {
                return Ok(r);
            }
        }

        let cwd = std::env::current_dir()?;
        let python = std::env::var("FRANIME_PYTHON")
            .ok()
            .or_else(|| {
                let venv = cwd.join(".venv").join("bin").join("python");
                venv.exists().then(|| venv.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "python3".to_string());

        let script_path = cwd.join("python").join("cf_solve.py");
        if !script_path.exists() {
            return Err(SidecarError::ScriptNotFound(script_path));
        }

        let headless = self.headless.load(Ordering::SeqCst);
        let mut child = Command::new(&python)
            .arg(&script_path)
            .arg("https://franime.fr")
            .env("FRANIME_HEADLESS", if headless { "1" } else { "0" })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| SidecarError::PythonNotFound(python.clone(), e))?;

        let stdin = child.stdin.take().ok_or_else(|| {
            SidecarError::Remote("stdin pipe absent".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            SidecarError::Remote("stdout pipe absent".into())
        })?;
        let stderr = child.stderr.take();
        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let level = if line.contains("error")
                        || line.contains("Error")
                        || line.contains("Traceback")
                        || line.contains("Exception")
                    {
                        crate::applog::LogLevel::Error
                    } else if line.contains("warn")
                        || line.contains("Warning")
                        || line.contains("skipped")
                        || line.contains("failed")
                    {
                        crate::applog::LogLevel::Warn
                    } else {
                        crate::applog::LogLevel::Info
                    };
                    crate::applog::log_event(crate::applog::LogSource::Python, level, line);
                }
            });
        }

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<SidecarReply>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let (ready_tx, ready_rx) = oneshot::channel::<Ready>();
        let ready_tx = Arc::new(Mutex::new(Some(ready_tx)));

        let pending_for_reader = pending.clone();
        let ready_for_reader = ready_tx.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let payload = match line.strip_prefix(RESULT_PREFIX) {
                    Some(p) => p,
                    None => {
                        crate::applog::log_event(
                            crate::applog::LogSource::Python,
                            crate::applog::LogLevel::Info,
                            line,
                        );
                        continue;
                    }
                };
                let resp: Response = match serde_json::from_str(payload) {
                    Ok(r) => r,
                    Err(e) => {
                        crate::applog::log_event(
                            crate::applog::LogSource::Sidecar,
                            crate::applog::LogLevel::Error,
                            format!("bad json: {} -- {}", e, payload),
                        );
                        continue;
                    }
                };

                if resp.msg_type.as_deref() == Some("ready") {
                    if let Some(tx) = ready_for_reader.lock().await.take() {
                        let _ = tx.send(Ready {
                            user_agent: resp.user_agent.unwrap_or_default(),
                            all_cookies: resp.all_cookies.unwrap_or_default(),
                        });
                    }
                    continue;
                }

                let Some(id) = resp.id else { continue };
                let mut map = pending_for_reader.lock().await;
                if let Some(tx) = map.remove(&id) {
                    let reply = if let Some(err) = resp.error {
                        SidecarReply::Error(err)
                    } else if let Some(src) = resp.iframe_src {
                        SidecarReply::IframeSrc(src)
                    } else if let Some(v) = resp.video_url {
                        SidecarReply::VideoUrl(v)
                    } else if resp.cf_clearance.is_some() {
                        SidecarReply::Refreshed {
                            user_agent: resp.user_agent.unwrap_or_default(),
                            cookies: resp.all_cookies.unwrap_or_default(),
                        }
                    } else if resp.pong == Some(true) || resp.ok == Some(true) {
                        SidecarReply::Ack
                    } else {
                        SidecarReply::Ack
                    };
                    let _ = tx.send(reply);
                }
            }
            crate::applog::log_event(
                crate::applog::LogSource::Sidecar,
                crate::applog::LogLevel::Warn,
                "reader task exiting (process closed stdout)",
            );
        });

        *guard = Some(SidecarInner {
            child,
            stdin,
            pending,
        });
        drop(guard);

        let ready = ready_rx.await.map_err(|_| {
            SidecarError::Remote("sidecar fermé avant ready".into())
        })?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.started_at.store(now, Ordering::SeqCst);
        self.cf_solves.fetch_add(1, Ordering::SeqCst);
        *self.cached_ready.lock().await = Some(ready.clone());
        Ok(ready)
    }

    pub async fn fetch_iframe(self: &Arc<Self>, url: &str) -> Result<String, SidecarError> {
        let r = self.send_cmd("fetch_iframe", Some(url)).await;
        match r {
            Ok(SidecarReply::IframeSrc(s)) if !s.is_empty() => {
                self.fetch_ok.fetch_add(1, Ordering::SeqCst);
                Ok(s)
            }
            Ok(SidecarReply::IframeSrc(_)) => {
                self.fetch_err.fetch_add(1, Ordering::SeqCst);
                Err(SidecarError::Remote("iframe src vide".into()))
            }
            Ok(SidecarReply::Error(e)) => {
                self.fetch_err.fetch_add(1, Ordering::SeqCst);
                Err(SidecarError::Remote(e))
            }
            Ok(other) => {
                self.fetch_err.fetch_add(1, Ordering::SeqCst);
                Err(SidecarError::Remote(format!(
                    "réponse inattendue: {:?}",
                    other
                )))
            }
            Err(e) => {
                self.fetch_err.fetch_add(1, Ordering::SeqCst);
                Err(e)
            }
        }
    }

    pub async fn capture_video_url(self: &Arc<Self>, url: &str) -> Result<String, SidecarError> {
        let r = self.send_cmd("capture_video_url", Some(url)).await;
        match r {
            Ok(SidecarReply::VideoUrl(v)) if !v.is_empty() => {
                self.fetch_ok.fetch_add(1, Ordering::SeqCst);
                Ok(v)
            }
            Ok(SidecarReply::VideoUrl(_)) => {
                self.fetch_err.fetch_add(1, Ordering::SeqCst);
                Err(SidecarError::Remote("video url vide".into()))
            }
            Ok(SidecarReply::Error(e)) => {
                self.fetch_err.fetch_add(1, Ordering::SeqCst);
                Err(SidecarError::Remote(e))
            }
            Ok(other) => {
                self.fetch_err.fetch_add(1, Ordering::SeqCst);
                Err(SidecarError::Remote(format!(
                    "réponse inattendue: {:?}",
                    other
                )))
            }
            Err(e) => {
                self.fetch_err.fetch_add(1, Ordering::SeqCst);
                Err(e)
            }
        }
    }

    pub async fn refresh_cf(self: &Arc<Self>) -> Result<Ready, SidecarError> {
        self.refresh_calls.fetch_add(1, Ordering::SeqCst);
        match self.send_cmd("refresh_cf", None).await? {
            SidecarReply::Refreshed { user_agent, cookies } => {
                self.cf_solves.fetch_add(1, Ordering::SeqCst);
                let r = Ready {
                    user_agent,
                    all_cookies: cookies,
                };
                *self.cached_ready.lock().await = Some(r.clone());
                Ok(r)
            }
            SidecarReply::Error(e) => Err(SidecarError::Remote(e)),
            other => Err(SidecarError::Remote(format!(
                "réponse inattendue: {:?}",
                other
            ))),
        }
    }

    async fn send_cmd(
        self: &Arc<Self>,
        cmd: &str,
        url: Option<&str>,
    ) -> Result<SidecarReply, SidecarError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();

        let payload = match url {
            Some(u) => serde_json::json!({"id": id, "cmd": cmd, "url": u}),
            None => serde_json::json!({"id": id, "cmd": cmd}),
        };
        let mut line = serde_json::to_string(&payload)?;
        line.push('\n');

        {
            let mut guard = self.inner.lock().await;
            let inner = guard.as_mut().ok_or(SidecarError::NotStarted)?;
            inner.pending.lock().await.insert(id, tx);
            inner.stdin.write_all(line.as_bytes()).await?;
            inner.stdin.flush().await?;
        }

        rx.await.map_err(|_| {
            SidecarError::Remote("sidecar fermé avant réponse".into())
        })
    }
}
