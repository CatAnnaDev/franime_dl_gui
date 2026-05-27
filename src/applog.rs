use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogSource {
    App,
    Sidecar,
    Python,
}

impl LogSource {
    pub fn short(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Sidecar => "sidecar",
            Self::Python => "py",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn short(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "err",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub ts: SystemTime,
    pub source: LogSource,
    pub level: LogLevel,
    pub message: String,
}

pub struct AppLog {
    entries: Mutex<VecDeque<LogEntry>>,
    max: usize,
}

impl AppLog {
    pub fn new(max: usize) -> Self {
        Self {
            entries: Mutex::new(VecDeque::with_capacity(max.min(64))),
            max,
        }
    }
    pub fn push(&self, entry: LogEntry) {
        let mut g = self.entries.lock().unwrap();
        while g.len() >= self.max {
            g.pop_front();
        }
        g.push_back(entry);
    }
    pub fn snapshot(&self) -> Vec<LogEntry> {
        self.entries.lock().unwrap().iter().cloned().collect()
    }
    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

static APP_LOG: OnceLock<Arc<AppLog>> = OnceLock::new();

pub fn instance() -> Arc<AppLog> {
    APP_LOG
        .get_or_init(|| Arc::new(AppLog::new(2000)))
        .clone()
}

pub fn log_event(source: LogSource, level: LogLevel, message: impl Into<String>) {
    let m = message.into();
    instance().push(LogEntry {
        ts: SystemTime::now(),
        source,
        level,
        message: m.clone(),
    });
    eprintln!("[{}/{}] {}", source.short(), level.short(), m);
}
