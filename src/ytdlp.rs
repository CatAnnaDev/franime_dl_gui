use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum YtDlpError {
    #[error("yt-dlp introuvable: {0}")]
    NotFound(std::io::Error),
    #[error("yt-dlp a renvoyé code {0:?} — {1}")]
    NonZero(Option<i32>, String),
    #[error("yt-dlp n'a rien retourné")]
    Empty,
    #[error("Timeout yt-dlp après {0}s")]
    Timeout(u64),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
}

pub fn resolve_binary() -> String {
    if let Ok(p) = std::env::current_dir() {
        for c in [
            p.join(".venv").join("bin").join("yt-dlp"),
            p.join(".venv").join("Scripts").join("yt-dlp.exe"),
        ] {
            if c.exists() {
                return c.to_string_lossy().into_owned();
            }
        }
    }
    if let Ok(v) = std::env::var("FRANIME_YTDLP") {
        if !v.is_empty() {
            return v;
        }
    }
    "yt-dlp".to_string()
}

pub async fn extract_url(iframe_url: &str, referer: &str) -> Result<String, YtDlpError> {
    let bin = resolve_binary();
    let timeout_secs = 30u64;
    let mut cmd = Command::new(&bin);
    cmd.arg("--no-warnings")
        .arg("--quiet")
        .arg("--get-url")
        .arg("--no-playlist")
        .arg("--no-check-certificate");
    if !referer.is_empty() {
        cmd.arg("--referer").arg(referer);
    }
    cmd.arg(iframe_url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = cmd.spawn().map_err(YtDlpError::NotFound)?;
    let output =
        match tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
            .await
        {
            Ok(r) => r?,
            Err(_) => return Err(YtDlpError::Timeout(timeout_secs)),
        };

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr)
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
        return Err(YtDlpError::NonZero(output.status.code(), err));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let url = stdout
        .lines()
        .filter(|l| l.starts_with("http"))
        .last()
        .map(|s| s.trim().to_string());
    url.filter(|s| !s.is_empty()).ok_or(YtDlpError::Empty)
}
