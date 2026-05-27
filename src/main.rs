use crate::animes_api::{FullAnimeslist, Root2};
use crate::downloader::{
    sanitize_path_segment, CookieStore, DownloadEvent, DownloadManager, DownloadStatus as DlStatus,
    DownloadTask, CF_COOKIE_KEY, FRANIME_CF_CLEARANCE_FALLBACK,
};
use crate::url_fetcher::UrlFetcher;
use eframe::egui;
use egui::{Color32, ColorImage, RichText, Vec2};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::runtime::Runtime;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::RwLock;

mod anikuro;
mod animes_api;
mod applog;
mod cf_sidecar;
mod consumet;
mod downloader;
mod uniquestream;
mod url_fetcher;
mod voiranime;
mod ytdlp;

#[derive(Debug, Clone, PartialEq)]
pub enum DownloadStatus {
    NotDownloaded,
    Downloading(f32),
    Downloaded,
}

#[derive(Debug, Clone, Default)]
pub struct UserNote {
    pub rating: Option<f32>,
    pub comment: Option<String>,
    pub status: Option<String>,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct UsStoredAnime {
    pub content_id: String,
    pub json_data: String,
    pub detail_json: Option<String>,
    pub episodes_json: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VaStoredAnime {
    pub slug: String,
    pub json_data: String,
    pub episodes_json: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UserStatus {
    AVoir,
    EnCours,
    Pause,
    Termine,
    Abandonne,
}

impl UserStatus {
    fn as_db_key(self) -> &'static str {
        match self {
            Self::AVoir => "a_voir",
            Self::EnCours => "en_cours",
            Self::Pause => "pause",
            Self::Termine => "termine",
            Self::Abandonne => "abandonne",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::AVoir => "À voir",
            Self::EnCours => "En cours",
            Self::Pause => "Pause",
            Self::Termine => "Terminé",
            Self::Abandonne => "Abandonné",
        }
    }
    fn color(self) -> Color32 {
        match self {
            Self::AVoir => Color32::from_rgb(150, 150, 160),
            Self::EnCours => Color32::from_rgb(139, 233, 253),
            Self::Pause => Color32::from_rgb(241, 196, 15),
            Self::Termine => Color32::from_rgb(80, 250, 123),
            Self::Abandonne => Color32::from_rgb(255, 85, 85),
        }
    }
    fn all() -> [Self; 5] {
        [
            Self::AVoir,
            Self::EnCours,
            Self::Pause,
            Self::Termine,
            Self::Abandonne,
        ]
    }
    fn from_db_key(s: &str) -> Option<Self> {
        match s {
            "a_voir" => Some(Self::AVoir),
            "en_cours" => Some(Self::EnCours),
            "pause" => Some(Self::Pause),
            "termine" => Some(Self::Termine),
            "abandonne" => Some(Self::Abandonne),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppSettings {
    pub max_concurrent_downloads: usize,
    pub max_concurrent_extractions: usize,
    pub preferred_lecteur_host: String,
    pub download_dir: String,
    pub chrome_headless: bool,
    pub naming_format: String,
    pub skip_existing: bool,
    pub theme_dark: bool,
    pub notifications_enabled: bool,
    pub sidecar_warmup: bool,
    pub consumet_base_url: String,
    pub consumet_provider: String,
    pub consumet_enabled: bool,
    pub consumet_auto_fallback: bool,
    pub anikuro_enabled: bool,
    pub anikuro_provider: String,
    pub anikuro_auto_fallback: bool,
    pub anikuro_prefer_dub: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            max_concurrent_downloads: 4,
            max_concurrent_extractions: 1,
            preferred_lecteur_host: String::new(),
            download_dir: String::new(),
            chrome_headless: false,
            naming_format: "plex".to_string(),
            skip_existing: true,
            theme_dark: true,
            notifications_enabled: true,
            sidecar_warmup: false,
            consumet_base_url: String::new(),
            consumet_provider: "gogoanime".to_string(),
            consumet_enabled: false,
            consumet_auto_fallback: false,
            anikuro_enabled: true,
            anikuro_provider: "animepahe".to_string(),
            anikuro_auto_fallback: true,
            anikuro_prefer_dub: false,
        }
    }
}

impl AppSettings {
    fn load(db: &Database) -> Self {
        let s = Self::default();
        Self {
            max_concurrent_downloads: db
                .get_setting("max_concurrent_downloads")
                .ok()
                .flatten()
                .and_then(|v| v.parse().ok())
                .unwrap_or(s.max_concurrent_downloads),
            max_concurrent_extractions: db
                .get_setting("max_concurrent_extractions")
                .ok()
                .flatten()
                .and_then(|v| v.parse().ok())
                .unwrap_or(s.max_concurrent_extractions),
            preferred_lecteur_host: db
                .get_setting("preferred_lecteur_host")
                .ok()
                .flatten()
                .unwrap_or(s.preferred_lecteur_host),
            download_dir: db
                .get_setting("download_dir")
                .ok()
                .flatten()
                .unwrap_or(s.download_dir),
            chrome_headless: db
                .get_setting("chrome_headless")
                .ok()
                .flatten()
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(s.chrome_headless),
            naming_format: db
                .get_setting("naming_format")
                .ok()
                .flatten()
                .unwrap_or(s.naming_format),
            skip_existing: db
                .get_setting("skip_existing")
                .ok()
                .flatten()
                .map(|v| v == "1")
                .unwrap_or(s.skip_existing),
            theme_dark: db
                .get_setting("theme_dark")
                .ok()
                .flatten()
                .map(|v| v == "1")
                .unwrap_or(s.theme_dark),
            notifications_enabled: db
                .get_setting("notifications_enabled")
                .ok()
                .flatten()
                .map(|v| v == "1")
                .unwrap_or(s.notifications_enabled),
            sidecar_warmup: db
                .get_setting("sidecar_warmup")
                .ok()
                .flatten()
                .map(|v| v == "1")
                .unwrap_or(s.sidecar_warmup),
            consumet_base_url: db
                .get_setting("consumet_base_url")
                .ok()
                .flatten()
                .unwrap_or(s.consumet_base_url),
            consumet_provider: db
                .get_setting("consumet_provider")
                .ok()
                .flatten()
                .unwrap_or(s.consumet_provider),
            consumet_enabled: db
                .get_setting("consumet_enabled")
                .ok()
                .flatten()
                .map(|v| v == "1")
                .unwrap_or(s.consumet_enabled),
            consumet_auto_fallback: db
                .get_setting("consumet_auto_fallback")
                .ok()
                .flatten()
                .map(|v| v == "1")
                .unwrap_or(s.consumet_auto_fallback),
            anikuro_enabled: db
                .get_setting("anikuro_enabled")
                .ok()
                .flatten()
                .map(|v| v == "1")
                .unwrap_or(s.anikuro_enabled),
            anikuro_provider: db
                .get_setting("anikuro_provider")
                .ok()
                .flatten()
                .unwrap_or(s.anikuro_provider),
            anikuro_auto_fallback: db
                .get_setting("anikuro_auto_fallback")
                .ok()
                .flatten()
                .map(|v| v == "1")
                .unwrap_or(s.anikuro_auto_fallback),
            anikuro_prefer_dub: db
                .get_setting("anikuro_prefer_dub")
                .ok()
                .flatten()
                .map(|v| v == "1")
                .unwrap_or(s.anikuro_prefer_dub),
        }
    }

    fn save(&self, db: &Database) {
        let _ = db.set_setting(
            "max_concurrent_downloads",
            &self.max_concurrent_downloads.to_string(),
        );
        let _ = db.set_setting(
            "max_concurrent_extractions",
            &self.max_concurrent_extractions.to_string(),
        );
        let _ = db.set_setting("preferred_lecteur_host", &self.preferred_lecteur_host);
        let _ = db.set_setting("download_dir", &self.download_dir);
        let _ = db.set_setting(
            "chrome_headless",
            if self.chrome_headless { "1" } else { "0" },
        );
        let _ = db.set_setting("naming_format", &self.naming_format);
        let _ = db.set_setting("skip_existing", if self.skip_existing { "1" } else { "0" });
        let _ = db.set_setting("theme_dark", if self.theme_dark { "1" } else { "0" });
        let _ = db.set_setting(
            "notifications_enabled",
            if self.notifications_enabled { "1" } else { "0" },
        );
        let _ = db.set_setting(
            "sidecar_warmup",
            if self.sidecar_warmup { "1" } else { "0" },
        );
        let _ = db.set_setting("consumet_base_url", &self.consumet_base_url);
        let _ = db.set_setting("consumet_provider", &self.consumet_provider);
        let _ = db.set_setting(
            "consumet_enabled",
            if self.consumet_enabled { "1" } else { "0" },
        );
        let _ = db.set_setting(
            "consumet_auto_fallback",
            if self.consumet_auto_fallback { "1" } else { "0" },
        );
        let _ = db.set_setting(
            "anikuro_enabled",
            if self.anikuro_enabled { "1" } else { "0" },
        );
        let _ = db.set_setting("anikuro_provider", &self.anikuro_provider);
        let _ = db.set_setting(
            "anikuro_auto_fallback",
            if self.anikuro_auto_fallback { "1" } else { "0" },
        );
        let _ = db.set_setting(
            "anikuro_prefer_dub",
            if self.anikuro_prefer_dub { "1" } else { "0" },
        );
    }

    fn effective_download_dir(&self) -> std::path::PathBuf {
        if self.download_dir.is_empty() {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        } else {
            std::path::PathBuf::from(&self.download_dir)
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnimeDisplay {
    pub anime: Root2,
    pub download_status: DownloadStatus,
    pub has_vo: bool,
    pub has_vf: bool,
    pub image_loaded: bool,
    pub expanded: bool,
    pub user_rating: Option<f32>,
    pub user_comment: String,
    pub is_downloaded: bool,
    pub user_status: Option<UserStatus>,
    pub watched_eps: std::collections::HashSet<(usize, usize)>,
    pub user_tags: Vec<String>,
    pub tag_input: String,
    pub source: AnimeSource,
    pub us_content_id: Option<String>,
    pub us_loaded_episodes: bool,
    pub va_slug: Option<String>,
    pub va_loaded_episodes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimeSource {
    Franime,
    Uniquestream,
    Voiranime,
}

impl AnimeDisplay {
    fn new(anime: Root2) -> Self {
        let (has_vo, has_vf) = Self::check_languages(&anime);
        Self {
            anime,
            download_status: DownloadStatus::NotDownloaded,
            has_vo,
            has_vf,
            image_loaded: false,
            expanded: false,
            user_rating: None,
            user_comment: String::new(),
            is_downloaded: false,
            user_status: None,
            watched_eps: std::collections::HashSet::new(),
            user_tags: Vec::new(),
            tag_input: String::new(),
            source: AnimeSource::Franime,
            us_content_id: None,
            us_loaded_episodes: false,
            va_slug: None,
            va_loaded_episodes: false,
        }
    }

    fn from_va(series: &voiranime::VaSeries) -> Self {
        let id = voiranime::va_id_from(&series.slug);
        let mut a = Root2::default();
        a.id = id;
        a.title = series.title.clone();
        a.title_o = String::new();
        a.affiche = series.image.clone().unwrap_or_default();
        a.affiche_small = series.image.clone();
        a.description = String::new();
        a.note = String::new();
        a.start_date = String::new();
        a.status = String::new();
        a.nsfw = false;
        a.source_url = series.url.clone();
        a.themes = Vec::new();
        a.saisons = Vec::new();
        let mut d = AnimeDisplay::new(a);
        d.has_vo = !series.slug.ends_with("-vf");
        d.has_vf = series.slug.ends_with("-vf");
        d.source = AnimeSource::Voiranime;
        d.va_slug = Some(series.slug.clone());
        d
    }

    fn from_us(series: &uniquestream::BrowseSeries) -> Self {
        let id = us_id_from(&series.content_id);
        let mut a = Root2::default();
        a.id = id;
        a.title = series.title.clone();
        a.title_o = String::new();
        a.affiche = series
            .image
            .clone()
            .unwrap_or_default();
        a.affiche_small = series.image.clone();
        a.description = series.info.clone().unwrap_or_default();
        a.note = series
            .score
            .map(|s| format!("{:.1}", s))
            .unwrap_or_default();
        a.start_date = series.year.map(|y| y.to_string()).unwrap_or_default();
        a.status = series.status.clone().unwrap_or_default();
        a.nsfw = false;
        a.source_url = format!(
            "https://anime.uniquestream.net/series/{}",
            series.content_id
        );
        a.themes = Vec::new();
        a.saisons = Vec::new();
        let mut d = AnimeDisplay::new(a);
        d.has_vo = series.subbed;
        d.has_vf = series.dubbed;
        d.source = AnimeSource::Uniquestream;
        d.us_content_id = Some(series.content_id.clone());
        d
    }

    fn check_languages(anime: &Root2) -> (bool, bool) {
        let mut has_vo = false;
        let mut has_vf = false;

        for saison in &anime.saisons {
            for episode in &saison.episodes {
                if !episode.lang.vo.lecteurs.is_empty() {
                    has_vo = true;
                }
                if !episode.lang.vf.lecteurs.is_empty() {
                    has_vf = true;
                }
                if has_vo && has_vf {
                    return (true, true);
                }
            }
        }
        (has_vo, has_vf)
    }

    fn total_episodes(&self) -> usize {
        self.anime.saisons.iter().map(|s| s.episodes.len()).sum()
    }
}

struct Database {
    conn: Connection,
}

impl Database {
    fn new() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open("animes.db")?;
        // WAL improves read/write concurrency on the local DB file.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let db = Self { conn };
        db.init_tables()?;
        Ok(db)
    }

    fn init_tables(&self) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS animes (
                id REAL PRIMARY KEY,
                json_data TEXT NOT NULL,
                has_vo INTEGER NOT NULL,
                has_vf INTEGER NOT NULL,
                title TEXT NOT NULL,
                title_o TEXT,
                note TEXT,
                status TEXT,
                nsfw INTEGER,
                updated_date INTEGER,
                updated_date_vf INTEGER,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

        let _ = self
            .conn
            .execute("ALTER TABLE animes ADD COLUMN user_rating REAL", []);
        let _ = self
            .conn
            .execute("ALTER TABLE animes ADD COLUMN user_comment TEXT", []);
        let _ = self
            .conn
            .execute("ALTER TABLE animes ADD COLUMN user_status TEXT", []);
        let _ = self
            .conn
            .execute("ALTER TABLE animes ADD COLUMN user_finished_at INTEGER", []);

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS images (
                url TEXT PRIMARY KEY,
                image_data BLOB NOT NULL,
                width INTEGER,
                height INTEGER,
                downloaded_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS anime_downloads (
                anime_id REAL NOT NULL,
                season_idx INTEGER NOT NULL,
                ep_idx INTEGER NOT NULL,
                lang TEXT NOT NULL,
                file_path TEXT NOT NULL,
                completed_at INTEGER DEFAULT (strftime('%s', 'now')),
                PRIMARY KEY (anime_id, season_idx, ep_idx, lang)
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS watched_episodes (
                anime_id REAL NOT NULL,
                season_idx INTEGER NOT NULL,
                ep_idx INTEGER NOT NULL,
                watched_at INTEGER DEFAULT (strftime('%s', 'now')),
                PRIMARY KEY (anime_id, season_idx, ep_idx)
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS user_tags (
                anime_id REAL NOT NULL,
                tag TEXT NOT NULL,
                created_at INTEGER DEFAULT (strftime('%s', 'now')),
                PRIMARY KEY (anime_id, tag)
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_watched_anime ON watched_episodes(anime_id)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_tags_anime ON user_tags(anime_id)",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS uniquestream_animes (
                content_id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                image TEXT,
                description TEXT,
                year INTEGER,
                status TEXT,
                episodes_count INTEGER,
                seasons_count INTEGER,
                score REAL,
                audio_locales TEXT,
                subtitle_locales TEXT,
                json_data TEXT NOT NULL,
                detail_json TEXT,
                episodes_json TEXT,
                deep_done_at INTEGER,
                added_at INTEGER DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;
        let _ = self.conn.execute(
            "ALTER TABLE uniquestream_animes ADD COLUMN deep_done_at INTEGER",
            [],
        );
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_us_title ON uniquestream_animes(title)",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS voiranime_animes (
                slug TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                image TEXT,
                json_data TEXT NOT NULL,
                episodes_json TEXT,
                deep_done_at INTEGER,
                added_at INTEGER DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;
        let _ = self.conn.execute(
            "ALTER TABLE voiranime_animes ADD COLUMN deep_done_at INTEGER",
            [],
        );
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_va_title ON voiranime_animes(title)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_va_deep ON voiranime_animes(deep_done_at)",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_anime_title ON animes(title)",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_anime_updated ON animes(updated_date)",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_downloads_anime ON anime_downloads(anime_id)",
            [],
        )?;

        Ok(())
    }

    fn set_user_rating(&self, anime_id: f64, rating: Option<f32>) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE animes SET user_rating = ?1, updated_at = strftime('%s', 'now') WHERE id = ?2",
            params![rating, anime_id],
        )?;
        Ok(())
    }

    fn set_user_comment(&self, anime_id: f64, comment: Option<&str>) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE animes SET user_comment = ?1, updated_at = strftime('%s', 'now') WHERE id = ?2",
            params![comment, anime_id],
        )?;
        Ok(())
    }

    fn load_user_notes(&self) -> Result<HashMap<u64, UserNote>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, user_rating, user_comment, user_status, user_finished_at FROM animes
             WHERE user_rating IS NOT NULL
                OR user_comment IS NOT NULL
                OR user_status IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: f64 = row.get(0)?;
            let rating: Option<f32> = row.get(1)?;
            let comment: Option<String> = row.get(2)?;
            let status: Option<String> = row.get(3)?;
            let finished_at: Option<i64> = row.get(4)?;
            Ok((
                id.to_bits(),
                UserNote {
                    rating,
                    comment,
                    status,
                    finished_at,
                },
            ))
        })?;
        let mut map = HashMap::new();
        for r in rows {
            if let Ok((id, note)) = r {
                map.insert(id, note);
            }
        }
        Ok(map)
    }

    fn set_user_status(
        &self,
        anime_id: f64,
        status: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        let finished_at: Option<i64> = if status == Some("termine") {
            Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            )
        } else {
            None
        };
        self.conn.execute(
            "UPDATE animes SET user_status = ?1, user_finished_at = ?2, updated_at = strftime('%s', 'now') WHERE id = ?3",
            params![status, finished_at, anime_id],
        )?;
        Ok(())
    }

    fn load_watched(
        &self,
    ) -> Result<HashMap<u64, std::collections::HashSet<(usize, usize)>>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT anime_id, season_idx, ep_idx FROM watched_episodes")?;
        let rows = stmt.query_map([], |row| {
            let id: f64 = row.get(0)?;
            let s: i64 = row.get(1)?;
            let e: i64 = row.get(2)?;
            Ok((id.to_bits(), (s as usize, e as usize)))
        })?;
        let mut map: HashMap<u64, std::collections::HashSet<(usize, usize)>> = HashMap::new();
        for r in rows {
            if let Ok((id, key)) = r {
                map.entry(id).or_default().insert(key);
            }
        }
        Ok(map)
    }

    fn set_episode_watched(
        &self,
        anime_id: f64,
        season_idx: usize,
        ep_idx: usize,
        watched: bool,
    ) -> Result<(), rusqlite::Error> {
        if watched {
            self.conn.execute(
                "INSERT OR REPLACE INTO watched_episodes (anime_id, season_idx, ep_idx) VALUES (?1, ?2, ?3)",
                params![anime_id, season_idx as i64, ep_idx as i64],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM watched_episodes WHERE anime_id = ?1 AND season_idx = ?2 AND ep_idx = ?3",
                params![anime_id, season_idx as i64, ep_idx as i64],
            )?;
        }
        Ok(())
    }

    fn load_tags(&self) -> Result<HashMap<u64, Vec<String>>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT anime_id, tag FROM user_tags ORDER BY tag")?;
        let rows = stmt.query_map([], |row| {
            let id: f64 = row.get(0)?;
            let tag: String = row.get(1)?;
            Ok((id.to_bits(), tag))
        })?;
        let mut map: HashMap<u64, Vec<String>> = HashMap::new();
        for r in rows {
            if let Ok((id, tag)) = r {
                map.entry(id).or_default().push(tag);
            }
        }
        Ok(map)
    }

    fn add_tag(&self, anime_id: f64, tag: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT OR IGNORE INTO user_tags (anime_id, tag) VALUES (?1, ?2)",
            params![anime_id, tag],
        )?;
        Ok(())
    }

    fn remove_tag(&self, anime_id: f64, tag: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM user_tags WHERE anime_id = ?1 AND tag = ?2",
            params![anime_id, tag],
        )?;
        Ok(())
    }

    fn downloads_for_anime(
        &self,
        anime_id: f64,
    ) -> Result<Vec<(usize, usize, String, String)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT season_idx, ep_idx, lang, file_path FROM anime_downloads
             WHERE anime_id = ?1 ORDER BY season_idx, ep_idx",
        )?;
        let rows = stmt.query_map(params![anime_id], |row| {
            let s: i64 = row.get(0)?;
            let e: i64 = row.get(1)?;
            let l: String = row.get(2)?;
            let p: String = row.get(3)?;
            Ok((s as usize, e as usize, l, p))
        })?;
        let mut out = Vec::new();
        for r in rows {
            if let Ok(t) = r {
                out.push(t);
            }
        }
        Ok(out)
    }

    fn all_downloads(&self) -> Result<Vec<(f64, String)>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT anime_id, file_path FROM anime_downloads")?;
        let rows = stmt.query_map([], |row| {
            let id: f64 = row.get(0)?;
            let p: String = row.get(1)?;
            Ok((id, p))
        })?;
        let mut out = Vec::new();
        for r in rows {
            if let Ok(t) = r {
                out.push(t);
            }
        }
        Ok(out)
    }

    fn delete_download_entry(&self, file_path: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM anime_downloads WHERE file_path = ?1",
            params![file_path],
        )?;
        Ok(())
    }

    fn save_us_anime(&self, series: &uniquestream::BrowseSeries) -> Result<(), rusqlite::Error> {
        let json_data = serde_json::to_string(series).unwrap_or_default();
        let audio = series
            .audio_locales
            .as_ref()
            .map(|v| v.join(","))
            .unwrap_or_default();
        let subs = series
            .subtitle_locales
            .as_ref()
            .map(|v| v.join(","))
            .unwrap_or_default();
        self.conn.execute(
            "INSERT INTO uniquestream_animes (
                content_id, title, image, description, year, status,
                episodes_count, seasons_count, score, audio_locales, subtitle_locales, json_data
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(content_id) DO UPDATE SET
                title = excluded.title,
                image = excluded.image,
                description = excluded.description,
                year = excluded.year,
                status = excluded.status,
                episodes_count = excluded.episodes_count,
                seasons_count = excluded.seasons_count,
                score = excluded.score,
                audio_locales = excluded.audio_locales,
                subtitle_locales = excluded.subtitle_locales,
                json_data = excluded.json_data,
                updated_at = strftime('%s', 'now')",
            params![
                series.content_id,
                series.title,
                series.image,
                series.info,
                series.year,
                series.status,
                series.episodes_count,
                series.seasons_count,
                series.score,
                audio,
                subs,
                json_data,
            ],
        )?;
        Ok(())
    }

    fn load_us_animes_pending_deep(&self) -> Result<Vec<UsStoredAnime>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT content_id, json_data, detail_json, episodes_json FROM uniquestream_animes WHERE deep_done_at IS NULL ORDER BY title",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(UsStoredAnime {
                content_id: row.get(0)?,
                json_data: row.get(1)?,
                detail_json: row.get(2)?,
                episodes_json: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            if let Ok(a) = r {
                out.push(a);
            }
        }
        Ok(out)
    }

    fn mark_us_deep_done(&self, content_id: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE uniquestream_animes SET deep_done_at = strftime('%s', 'now'), updated_at = strftime('%s', 'now') WHERE content_id = ?1",
            params![content_id],
        )?;
        Ok(())
    }

    fn update_us_json(&self, content_id: &str, json_data: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE uniquestream_animes SET json_data = ?1, updated_at = strftime('%s', 'now') WHERE content_id = ?2",
            params![json_data, content_id],
        )?;
        Ok(())
    }

    fn save_us_detail(&self, content_id: &str, detail_json: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE uniquestream_animes SET detail_json = ?1, updated_at = strftime('%s', 'now') WHERE content_id = ?2",
            params![detail_json, content_id],
        )?;
        Ok(())
    }

    fn save_us_episodes(
        &self,
        content_id: &str,
        episodes_json: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE uniquestream_animes SET episodes_json = ?1, updated_at = strftime('%s', 'now') WHERE content_id = ?2",
            params![episodes_json, content_id],
        )?;
        Ok(())
    }

    fn save_va_anime(&self, series: &voiranime::VaSeries) -> Result<(), rusqlite::Error> {
        let json_data = serde_json::to_string(series).unwrap_or_default();
        self.conn.execute(
            "INSERT INTO voiranime_animes (slug, title, image, json_data)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(slug) DO UPDATE SET
                title = excluded.title,
                image = excluded.image,
                json_data = excluded.json_data,
                updated_at = strftime('%s', 'now')",
            params![series.slug, series.title, series.image, json_data],
        )?;
        Ok(())
    }

    fn save_va_episodes(
        &self,
        slug: &str,
        episodes_json: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE voiranime_animes SET episodes_json = ?1, updated_at = strftime('%s', 'now') WHERE slug = ?2",
            params![episodes_json, slug],
        )?;
        Ok(())
    }

    fn load_va_animes_pending_deep(&self) -> Result<Vec<VaStoredAnime>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT slug, json_data, episodes_json FROM voiranime_animes WHERE deep_done_at IS NULL ORDER BY title",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(VaStoredAnime {
                slug: row.get(0)?,
                json_data: row.get(1)?,
                episodes_json: row.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            if let Ok(a) = r {
                out.push(a);
            }
        }
        Ok(out)
    }

    fn mark_va_deep_done(&self, slug: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE voiranime_animes SET deep_done_at = strftime('%s', 'now'), updated_at = strftime('%s', 'now') WHERE slug = ?1",
            params![slug],
        )?;
        Ok(())
    }

    fn count_va_pending_deep(&self) -> Result<i64, rusqlite::Error> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM voiranime_animes WHERE deep_done_at IS NULL",
            [],
            |row| row.get(0),
        )?;
        Ok(n)
    }

    fn load_va_animes(&self) -> Result<Vec<VaStoredAnime>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT slug, json_data, episodes_json FROM voiranime_animes ORDER BY title",
        )?;
        let rows = stmt.query_map([], |row| {
            let slug: String = row.get(0)?;
            let json: String = row.get(1)?;
            let eps: Option<String> = row.get(2)?;
            Ok(VaStoredAnime {
                slug,
                json_data: json,
                episodes_json: eps,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            if let Ok(a) = r {
                out.push(a);
            }
        }
        Ok(out)
    }

    fn load_us_animes(&self) -> Result<Vec<UsStoredAnime>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT content_id, json_data, detail_json, episodes_json FROM uniquestream_animes ORDER BY title",
        )?;
        let rows = stmt.query_map([], |row| {
            let cid: String = row.get(0)?;
            let json: String = row.get(1)?;
            let detail: Option<String> = row.get(2)?;
            let eps: Option<String> = row.get(3)?;
            Ok(UsStoredAnime {
                content_id: cid,
                json_data: json,
                detail_json: detail,
                episodes_json: eps,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            if let Ok(a) = r {
                out.push(a);
            }
        }
        Ok(out)
    }

    fn download_history(&self) -> Result<Vec<(f64, usize, usize, String, i64)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT anime_id, season_idx, ep_idx, lang, completed_at FROM anime_downloads ORDER BY completed_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: f64 = row.get(0)?;
            let s: i64 = row.get(1)?;
            let e: i64 = row.get(2)?;
            let l: String = row.get(3)?;
            let t: i64 = row.get(4)?;
            Ok((id, s as usize, e as usize, l, t))
        })?;
        let mut out = Vec::new();
        for r in rows {
            if let Ok(t) = r {
                out.push(t);
            }
        }
        Ok(out)
    }

    fn record_download(
        &self,
        anime_id: f64,
        season_idx: usize,
        ep_idx: usize,
        lang: &str,
        file_path: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO anime_downloads (anime_id, season_idx, ep_idx, lang, file_path)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(anime_id, season_idx, ep_idx, lang) DO UPDATE SET
                file_path = excluded.file_path,
                completed_at = strftime('%s', 'now')",
            params![anime_id, season_idx as i64, ep_idx as i64, lang, file_path],
        )?;
        Ok(())
    }

    fn downloaded_anime_ids(&self) -> Result<std::collections::HashSet<u64>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT anime_id FROM anime_downloads")?;
        let rows = stmt.query_map([], |row| {
            let id: f64 = row.get(0)?;
            Ok(id.to_bits())
        })?;
        let mut set = std::collections::HashSet::new();
        for r in rows {
            if let Ok(id) = r {
                set.insert(id);
            }
        }
        Ok(set)
    }

    fn save_or_update_anime(&self, anime: &Root2) -> Result<(), rusqlite::Error> {
        let (has_vo, has_vf) = AnimeDisplay::check_languages(anime);
        let json_data = serde_json::to_string(anime).unwrap_or_default();

        self.conn.execute(
            "INSERT INTO animes (
                id, json_data, has_vo, has_vf, title, title_o,
                note, status, nsfw, updated_date, updated_date_vf
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(id) DO UPDATE SET
                json_data = excluded.json_data,
                has_vo = excluded.has_vo,
                has_vf = excluded.has_vf,
                title = excluded.title,
                title_o = excluded.title_o,
                note = excluded.note,
                status = excluded.status,
                nsfw = excluded.nsfw,
                updated_date = excluded.updated_date,
                updated_date_vf = excluded.updated_date_vf,
                updated_at = strftime('%s', 'now')",
            params![
                anime.id,
                json_data,
                has_vo as i32,
                has_vf as i32,
                anime.title,
                anime.title_o,
                anime.note,
                anime.status,
                anime.nsfw as i32,
                anime.updated_date,
                anime.updated_date_vf,
            ],
        )?;

        Ok(())
    }

    fn save_image(
        &self,
        url: &str,
        image_data: &[u8],
        width: u32,
        height: u32,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT OR REPLACE INTO images (url, image_data, width, height) VALUES (?1, ?2, ?3, ?4)",
            params![url, image_data, width as i32, height as i32],
        )?;
        Ok(())
    }

    fn get_image(&self, url: &str) -> Result<Option<Vec<u8>>, rusqlite::Error> {
        let result = self.conn.query_row(
            "SELECT image_data FROM images WHERE url = ?1",
            params![url],
            |row| row.get(0),
        );

        match result {
            Ok(data) => Ok(Some(data)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn get_setting(&self, key: &str) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
    }

    fn set_setting(&self, key: &str, value: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, strftime('%s', 'now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = strftime('%s', 'now')",
            params![key, value],
        )?;
        Ok(())
    }

    fn load_animes(&self) -> Result<Vec<Root2>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT json_data FROM animes ORDER BY title")?;

        let anime_iter = stmt.query_map([], |row| {
            let json_str: String = row.get(0)?;
            Ok(json_str)
        })?;

        let mut animes = Vec::new();
        for json_result in anime_iter {
            if let Ok(json_str) = json_result {
                if let Ok(anime) = serde_json::from_str::<Root2>(&json_str) {
                    animes.push(anime);
                }
            }
        }

        Ok(animes)
    }
}

pub struct AnimeDownloaderApp {
    animes: Vec<AnimeDisplay>,
    filtered_indices: Vec<usize>,
    search_query: String,
    lang_filter: LangFilter,
    view_mode: ViewMode,
    images: HashMap<String, egui::TextureHandle>,
    image_fetching: Arc<StdMutex<std::collections::HashSet<String>>>,
    image_missing: std::collections::HashSet<String>,
    image_loading: std::collections::HashSet<String>,
    image_load_tx: std::sync::mpsc::Sender<(String, Option<Vec<u8>>)>,
    image_load_rx: std::sync::mpsc::Receiver<(String, Option<Vec<u8>>)>,
    db: Arc<AsyncMutex<Database>>,
    runtime: Runtime,
    is_syncing: Arc<AtomicBool>,
    sync_done_rx: std::sync::mpsc::Receiver<SyncOutcome>,
    sync_done_tx: std::sync::mpsc::Sender<SyncOutcome>,
    sync_status: String,
    manager: Arc<DownloadManager>,
    task_view: Arc<StdMutex<Vec<DownloadTask>>>,
    task_originals: Arc<StdMutex<HashMap<String, OriginalRequest>>>,
    fetcher: Arc<UrlFetcher>,
    cf_refreshing: Arc<AtomicBool>,
    settings: AppSettings,
    settings_pending: AppSettings,
    show_settings: bool,
    downloads_filter: DownloadsFilter,
    show_close_confirm: bool,
    confirmed_close: bool,
    logs_filter_source: Option<applog::LogSource>,
    logs_filter_level: Option<applog::LogLevel>,
    logs_filter_query: String,
    logs_autoscroll: bool,
    us_load_tx: std::sync::mpsc::Sender<UsLoadResult>,
    us_load_rx: std::sync::mpsc::Receiver<UsLoadResult>,
    va_load_tx: std::sync::mpsc::Sender<VaLoadResult>,
    va_load_rx: std::sync::mpsc::Receiver<VaLoadResult>,
    va_loading: Arc<StdMutex<std::collections::HashSet<String>>>,
    va_episode_urls: Arc<StdMutex<HashMap<(u64, usize, usize), String>>>,
    va_episode_sources: Arc<StdMutex<HashMap<(u64, usize, usize), Vec<voiranime::VaSource>>>>,
    us_loading: Arc<StdMutex<std::collections::HashSet<String>>>,
    us_episode_ids: Arc<StdMutex<HashMap<(u64, usize, usize), String>>>,
    us_audio_locales: Arc<StdMutex<HashMap<u64, Vec<String>>>>,
    us_movies: Arc<StdMutex<std::collections::HashSet<String>>>,
    selected_themes: std::collections::BTreeSet<String>,
    theme_filter_mode: ThemeFilterMode,
    min_user_rating: f32,
    only_downloaded: bool,
    hide_nsfw: bool,
    selected_statuses: std::collections::HashSet<UserStatus>,
    selected_user_tags: std::collections::BTreeSet<String>,
    all_themes_cache: Vec<String>,
    all_user_tags_cache: Vec<String>,
    sort_mode: SortMode,
    sort_descending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ThemeFilterMode {
    Any,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SortMode {
    TitleAlpha,
    StartDate,
    UserRating,
    SiteRating,
    EpisodesCount,
    SeasonsCount,
}

impl SortMode {
    fn label(self) -> &'static str {
        match self {
            Self::TitleAlpha => "Titre",
            Self::StartDate => "Date sortie",
            Self::UserRating => "Ma note",
            Self::SiteRating => "Note site",
            Self::EpisodesCount => "Nb épisodes",
            Self::SeasonsCount => "Nb saisons",
        }
    }
    fn all() -> [Self; 6] {
        [
            Self::TitleAlpha,
            Self::StartDate,
            Self::UserRating,
            Self::SiteRating,
            Self::EpisodesCount,
            Self::SeasonsCount,
        ]
    }
}

enum SyncOutcome {
    Success { saved: usize, total: usize },
    Failure(String),
}

#[derive(Debug)]
struct UsLoadResult {
    content_id: String,
    anime_id_bits: u64,
    cached: Result<uniquestream::UsCachedEpisodes, String>,
}

#[derive(Debug)]
struct VaLoadResult {
    slug: String,
    anime_id_bits: u64,
    cached: Result<voiranime::VaCachedEpisodes, String>,
}

#[derive(Debug, Clone)]
struct OriginalRequest {
    anime: Root2,
    season_idx: usize,
    ep_idx: usize,
    lang: &'static str,
    lecteurs_to_try: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq)]
enum LangFilter {
    All,
    VO,
    VF,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ViewMode {
    Catalogue,
    MaListe,
    Telechargements,
    Logs,
    Stats,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DownloadsFilter {
    All,
    Active,
    Failed,
    Done,
}

impl AnimeDownloaderApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals.window_corner_radius = 10.0_f32.into();
        style.visuals.widgets.noninteractive.corner_radius = 8.0_f32.into();
        cc.egui_ctx.set_style(style);

        let db = Arc::new(AsyncMutex::new(
            Database::new().expect("Failed to create database"),
        ));

        let runtime = Runtime::new().expect("Failed to create tokio runtime");

        let us_episode_ids: Arc<StdMutex<HashMap<(u64, usize, usize), String>>> =
            Arc::new(StdMutex::new(HashMap::new()));
        let us_audio_locales: Arc<StdMutex<HashMap<u64, Vec<String>>>> =
            Arc::new(StdMutex::new(HashMap::new()));

        let (settings, animes): (AppSettings, Vec<AnimeDisplay>) = {
            let db = db.clone();
            let us_episode_ids_init = us_episode_ids.clone();
            let us_audio_locales_init = us_audio_locales.clone();
            let us_movies_init: Arc<StdMutex<std::collections::HashSet<String>>> =
                Arc::new(StdMutex::new(std::collections::HashSet::new()));
            runtime.block_on(async move {
                let guard = db.lock().await;
                let settings = AppSettings::load(&guard);
                let notes = guard.load_user_notes().unwrap_or_default();
                let downloaded = guard.downloaded_anime_ids().unwrap_or_default();
                let watched = guard.load_watched().unwrap_or_default();
                let tags = guard.load_tags().unwrap_or_default();
                let us_rows = guard.load_us_animes().unwrap_or_default();
                let mut animes: Vec<AnimeDisplay> = guard
                    .load_animes()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|a| {
                        let mut d = AnimeDisplay::new(a);
                        let key = d.anime.id.to_bits();
                        if let Some(n) = notes.get(&key) {
                            d.user_rating = n.rating;
                            d.user_comment = n.comment.clone().unwrap_or_default();
                            d.user_status =
                                n.status.as_deref().and_then(UserStatus::from_db_key);
                        }
                        d.is_downloaded = downloaded.contains(&key);
                        if let Some(set) = watched.get(&key) {
                            d.watched_eps = set.clone();
                        }
                        if let Some(t) = tags.get(&key) {
                            d.user_tags = t.clone();
                        }
                        d
                    })
                    .collect();
                for row in &us_rows {
                    if let Ok(series) =
                        serde_json::from_str::<uniquestream::BrowseSeries>(&row.json_data)
                    {
                        let mut d = AnimeDisplay::from_us(&series);
                        let key = d.anime.id.to_bits();
                        if let Some(n) = notes.get(&key) {
                            d.user_rating = n.rating;
                            d.user_comment = n.comment.clone().unwrap_or_default();
                            d.user_status =
                                n.status.as_deref().and_then(UserStatus::from_db_key);
                        }
                        d.is_downloaded = downloaded.contains(&key);
                        if let Some(set) = watched.get(&key) {
                            d.watched_eps = set.clone();
                        }
                        if let Some(t) = tags.get(&key) {
                            d.user_tags = t.clone();
                        }
                        if let Some(eps_json) = &row.episodes_json {
                            if let Ok(cached) = serde_json::from_str::<
                                uniquestream::UsCachedEpisodes,
                            >(eps_json)
                            {
                                apply_us_cached_to_anime(
                                    &mut d,
                                    &cached,
                                    &us_episode_ids_init,
                                    &us_audio_locales_init,
                                    &us_movies_init,
                                );
                            }
                        }
                        animes.push(d);
                    }
                }
                animes.sort_by(|a, b| a.anime.title.to_lowercase().cmp(&b.anime.title.to_lowercase()));
                (settings, animes)
            })
        };

        let tasks: Arc<RwLock<Vec<DownloadTask>>> = Arc::new(RwLock::new(Vec::new()));

        // Bootstrap cf_clearance: prefer the value persisted in the DB, else the embedded fallback.
        let initial_cookie = {
            let db = db.clone();
            runtime
                .block_on(async move {
                    let guard = db.lock().await;
                    guard.get_setting(CF_COOKIE_KEY).ok().flatten()
                })
                .unwrap_or_else(|| FRANIME_CF_CLEARANCE_FALLBACK.to_string())
        };
        let (cookies, mut cookie_save_rx) = CookieStore::new(initial_cookie);

        // Persist any refreshed cookie to the DB.
        {
            let db = db.clone();
            runtime.spawn(async move {
                while let Some(value) = cookie_save_rx.recv().await {
                    let guard = db.lock().await;
                    if let Err(e) = guard.set_setting(CF_COOKIE_KEY, &value) {
                        eprintln!("Échec persistance cf_clearance: {}", e);
                    }
                }
            });
        }

        let (manager, mut updates) = DownloadManager::new(
            settings.chrome_headless,
            settings.max_concurrent_downloads.max(1),
            tasks.clone(),
            cookies.clone(),
        );
        let cf_refreshing = manager.cf_refreshing();
        let manager = Arc::new(manager);

        let task_view: Arc<StdMutex<Vec<DownloadTask>>> = Arc::new(StdMutex::new(Vec::new()));
        let task_originals: Arc<StdMutex<HashMap<String, OriginalRequest>>> =
            Arc::new(StdMutex::new(HashMap::new()));

        {
            let task_view = task_view.clone();
            let task_originals_ev = task_originals.clone();
            let db_ev = db.clone();
            runtime.spawn(async move {
                while let Some(event) = updates.recv().await {
                    let recorded_completion = matches!(&event,
                        DownloadEvent::Updated(t) if matches!(t.status, downloader::DownloadStatus::Completed));
                    let task_id_for_record = match &event {
                        DownloadEvent::Updated(t) => Some(t.id.clone()),
                        _ => None,
                    };
                    {
                        let mut view = task_view.lock().unwrap();
                        match event {
                            DownloadEvent::Updated(task) => {
                                if let Some(slot) = view.iter_mut().find(|t| t.id == task.id) {
                                    slot.status = task.status;
                                    if !task.url.is_empty() {
                                        slot.url = task.url;
                                    }
                                    slot.output_path = task.output_path;
                                    if task.host.is_some() {
                                        slot.host = task.host;
                                    }
                                    if !task.attempted_lecteurs.is_empty() {
                                        slot.attempted_lecteurs = task.attempted_lecteurs;
                                    }
                                } else {
                                    view.push(task);
                                }
                            }
                            DownloadEvent::Removed(id) => {
                                view.retain(|t| t.id != id);
                            }
                        }
                    }
                    if recorded_completion {
                        if let Some(tid) = task_id_for_record {
                            let original = task_originals_ev.lock().unwrap().get(&tid).cloned();
                            let path = task_view
                                .lock()
                                .unwrap()
                                .iter()
                                .find(|t| t.id == tid)
                                .map(|t| t.output_path.clone());
                            if let (Some(orig), Some(p)) = (original, path) {
                                let guard = db_ev.lock().await;
                                let _ = guard.record_download(
                                    orig.anime.id,
                                    orig.season_idx,
                                    orig.ep_idx,
                                    orig.lang,
                                    &p.to_string_lossy(),
                                );
                            }
                        }
                    }
                }
            });
        }

        let (sync_done_tx, sync_done_rx) = std::sync::mpsc::channel();
        let (us_load_tx, us_load_rx) = std::sync::mpsc::channel::<UsLoadResult>();
        let (va_load_tx, va_load_rx) = std::sync::mpsc::channel::<VaLoadResult>();
        let (image_load_tx, image_load_rx) =
            std::sync::mpsc::channel::<(String, Option<Vec<u8>>)>();

        let filtered_indices = (0..animes.len()).collect();

        let mut out = Self {
            animes,
            filtered_indices,
            search_query: String::new(),
            lang_filter: LangFilter::All,
            view_mode: ViewMode::Catalogue,
            images: HashMap::new(),
            image_fetching: Arc::new(StdMutex::new(std::collections::HashSet::new())),
            image_missing: std::collections::HashSet::new(),
            image_loading: std::collections::HashSet::new(),
            image_load_tx,
            image_load_rx,
            db,
            runtime,
            is_syncing: Arc::new(AtomicBool::new(false)),
            sync_done_rx,
            sync_done_tx,
            sync_status: String::new(),
            manager,
            task_view,
            task_originals,
            fetcher: Arc::new(UrlFetcher::new()),
            cf_refreshing,
            settings: settings.clone(),
            settings_pending: settings,
            show_settings: false,
            downloads_filter: DownloadsFilter::All,
            show_close_confirm: false,
            confirmed_close: false,
            us_load_tx: us_load_tx.clone(),
            us_load_rx,
            us_loading: Arc::new(StdMutex::new(std::collections::HashSet::new())),
            us_episode_ids,
            us_audio_locales,
            us_movies: Arc::new(StdMutex::new(std::collections::HashSet::new())),
            va_load_tx: va_load_tx.clone(),
            va_load_rx,
            va_loading: Arc::new(StdMutex::new(std::collections::HashSet::new())),
            va_episode_urls: Arc::new(StdMutex::new(HashMap::new())),
            va_episode_sources: Arc::new(StdMutex::new(HashMap::new())),
            logs_filter_source: None,
            logs_filter_level: None,
            logs_filter_query: String::new(),
            logs_autoscroll: true,
            selected_themes: std::collections::BTreeSet::new(),
            theme_filter_mode: ThemeFilterMode::Any,
            min_user_rating: 0.0,
            only_downloaded: false,
            hide_nsfw: false,
            selected_statuses: std::collections::HashSet::new(),
            selected_user_tags: std::collections::BTreeSet::new(),
            all_themes_cache: Vec::new(),
            all_user_tags_cache: Vec::new(),
            sort_mode: SortMode::TitleAlpha,
            sort_descending: false,
        };
        out.rebuild_themes_cache();
        out.rebuild_user_tags_cache();

        if out.settings.sidecar_warmup {
            let manager = out.manager.clone();
            out.runtime.spawn(async move {
                applog::log_event(
                    applog::LogSource::Sidecar,
                    applog::LogLevel::Info,
                    "préchauffage du sidecar au démarrage",
                );
                if let Err(e) = manager.warmup_sidecar().await {
                    applog::log_event(
                        applog::LogSource::Sidecar,
                        applog::LogLevel::Error,
                        format!("préchauffage échoué: {}", e),
                    );
                }
            });
        }

        out
    }

    fn rebuild_themes_cache(&mut self) {
        let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for a in &self.animes {
            for t in &a.anime.themes {
                if !t.is_empty() {
                    set.insert(t.clone());
                }
            }
        }
        self.all_themes_cache = set.into_iter().collect();
    }

    fn rebuild_user_tags_cache(&mut self) {
        let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for a in &self.animes {
            for t in &a.user_tags {
                set.insert(t.clone());
            }
        }
        self.all_user_tags_cache = set.into_iter().collect();
    }

    fn sync_from_api(&mut self, ctx: egui::Context) {
        if self.is_syncing.swap(true, Ordering::SeqCst) {
            return;
        }

        self.sync_status = "Synchronisation franime en cours…".to_string();

        let db = Arc::clone(&self.db);
        let tx = self.sync_done_tx.clone();
        let ctx_clone = ctx.clone();
        let is_syncing = self.is_syncing.clone();

        self.runtime.spawn(async move {
            let outcome = run_sync(db).await;
            is_syncing.store(false, Ordering::SeqCst);
            let _ = tx.send(outcome);
            ctx_clone.request_repaint();
        });
    }

    fn trigger_us_load(&self, anime_idx: usize) {
        let display = &self.animes[anime_idx];
        if display.source != AnimeSource::Uniquestream {
            return;
        }
        if display.us_loaded_episodes && !display.anime.saisons.is_empty() {
            return;
        }
        let Some(content_id) = display.us_content_id.clone() else {
            return;
        };
        let anime_id_bits = display.anime.id.to_bits();
        {
            let mut loading = self.us_loading.lock().unwrap();
            if loading.contains(&content_id) {
                return;
            }
            loading.insert(content_id.clone());
        }
        let tx = self.us_load_tx.clone();
        let db = self.db.clone();
        applog::log_event(
            applog::LogSource::App,
            applog::LogLevel::Info,
            format!("uniquestream load épisodes pour {}", content_id),
        );
        self.runtime.spawn(async move {
            let client = match uniquestream::UsClient::new() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(UsLoadResult {
                        content_id,
                        anime_id_bits,
                        cached: Err(format!("client: {}", e)),
                    });
                    return;
                }
            };
            let result = uniquestream::fetch_all_episodes(&client, &content_id).await;
            match result {
                Ok(cached) => {
                    if let Ok(json) = serde_json::to_string(&cached) {
                        let guard = db.lock().await;
                        let _ = guard.save_us_episodes(&content_id, &json);
                    }
                    let _ = tx.send(UsLoadResult {
                        content_id,
                        anime_id_bits,
                        cached: Ok(cached),
                    });
                }
                Err(e) => {
                    let _ = tx.send(UsLoadResult {
                        content_id,
                        anime_id_bits,
                        cached: Err(e.to_string()),
                    });
                }
            }
        });
    }

    fn trigger_va_load(&self, anime_idx: usize) {
        let display = &self.animes[anime_idx];
        if display.source != AnimeSource::Voiranime {
            return;
        }
        if display.va_loaded_episodes && !display.anime.saisons.is_empty() {
            return;
        }
        let Some(slug) = display.va_slug.clone() else {
            return;
        };
        let anime_id_bits = display.anime.id.to_bits();
        {
            let mut loading = self.va_loading.lock().unwrap();
            if loading.contains(&slug) {
                return;
            }
            loading.insert(slug.clone());
        }
        let tx = self.va_load_tx.clone();
        let db = self.db.clone();
        applog::log_event(
            applog::LogSource::App,
            applog::LogLevel::Info,
            format!("voiranime load épisodes pour {}", slug),
        );
        self.runtime.spawn(async move {
            let client = match voiranime::VaClient::new() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(VaLoadResult {
                        slug,
                        anime_id_bits,
                        cached: Err(format!("client: {}", e)),
                    });
                    return;
                }
            };
            let result = voiranime::fetch_all_episodes(&client, &slug).await;
            match result {
                Ok(cached) => {
                    if let Ok(json) = serde_json::to_string(&cached) {
                        let guard = db.lock().await;
                        let _ = guard.save_va_episodes(&slug, &json);
                    }
                    let _ = tx.send(VaLoadResult {
                        slug,
                        anime_id_bits,
                        cached: Ok(cached),
                    });
                }
                Err(e) => {
                    let _ = tx.send(VaLoadResult {
                        slug,
                        anime_id_bits,
                        cached: Err(e.to_string()),
                    });
                }
            }
        });
    }

    fn drain_va_load_results(&mut self) {
        while let Ok(result) = self.va_load_rx.try_recv() {
            self.va_loading.lock().unwrap().remove(&result.slug);
            let Some(idx) = self
                .animes
                .iter()
                .position(|a| a.anime.id.to_bits() == result.anime_id_bits)
            else {
                continue;
            };
            match result.cached {
                Ok(cached) => {
                    apply_va_cached_to_anime(
                        &mut self.animes[idx],
                        &cached,
                        &self.va_episode_urls,
                        &self.va_episode_sources,
                    );
                    applog::log_event(
                        applog::LogSource::App,
                        applog::LogLevel::Info,
                        format!(
                            "voiranime {} chargé : {} épisode(s)",
                            result.slug,
                            cached.episodes.len()
                        ),
                    );
                }
                Err(e) => {
                    applog::log_event(
                        applog::LogSource::App,
                        applog::LogLevel::Error,
                        format!("voiranime load {} KO: {}", result.slug, e),
                    );
                }
            }
        }
    }

    fn backfill_images(&mut self, ctx: egui::Context) {
        if self.is_syncing.swap(true, Ordering::SeqCst) {
            return;
        }
        self.sync_status = "Backfill des images en cours…".to_string();
        let db = Arc::clone(&self.db);
        let tx = self.sync_done_tx.clone();
        let ctx_clone = ctx.clone();
        let is_syncing = self.is_syncing.clone();
        self.runtime.spawn(async move {
            let outcome = run_image_backfill(db).await;
            is_syncing.store(false, Ordering::SeqCst);
            let _ = tx.send(outcome);
            ctx_clone.request_repaint();
        });
    }

    fn sync_voiranime(&mut self, ctx: egui::Context) {
        if self.is_syncing.swap(true, Ordering::SeqCst) {
            return;
        }
        self.sync_status = "Synchronisation voiranime en cours…".to_string();
        let db = Arc::clone(&self.db);
        let tx = self.sync_done_tx.clone();
        let ctx_clone = ctx.clone();
        let is_syncing = self.is_syncing.clone();
        self.runtime.spawn(async move {
            let outcome = run_va_sync(db).await;
            is_syncing.store(false, Ordering::SeqCst);
            let _ = tx.send(outcome);
            ctx_clone.request_repaint();
        });
    }

    fn drain_us_load_results(&mut self) {
        while let Ok(result) = self.us_load_rx.try_recv() {
            self.us_loading.lock().unwrap().remove(&result.content_id);
            let Some(idx) = self
                .animes
                .iter()
                .position(|a| a.anime.id.to_bits() == result.anime_id_bits)
            else {
                continue;
            };
            match result.cached {
                Ok(cached) => {
                    apply_us_cached_to_anime(
                        &mut self.animes[idx],
                        &cached,
                        &self.us_episode_ids,
                        &self.us_audio_locales,
                        &self.us_movies,
                    );
                    applog::log_event(
                        applog::LogSource::App,
                        applog::LogLevel::Info,
                        format!(
                            "uniquestream {} chargé : {} saison(s)",
                            result.content_id,
                            self.animes[idx].anime.saisons.len()
                        ),
                    );
                }
                Err(e) => {
                    applog::log_event(
                        applog::LogSource::App,
                        applog::LogLevel::Error,
                        format!(
                            "uniquestream load {} KO: {}",
                            result.content_id, e
                        ),
                    );
                }
            }
        }
    }

    fn sync_uniquestream(&mut self, ctx: egui::Context) {
        if self.is_syncing.swap(true, Ordering::SeqCst) {
            return;
        }
        self.sync_status = "Synchronisation uniquestream en cours…".to_string();
        let db = Arc::clone(&self.db);
        let tx = self.sync_done_tx.clone();
        let ctx_clone = ctx.clone();
        let is_syncing = self.is_syncing.clone();
        self.runtime.spawn(async move {
            let outcome = run_us_sync(db).await;
            is_syncing.store(false, Ordering::SeqCst);
            let _ = tx.send(outcome);
            ctx_clone.request_repaint();
        });
    }

    fn drain_sync_results(&mut self) {
        while let Ok(outcome) = self.sync_done_rx.try_recv() {
            match outcome {
                SyncOutcome::Success { saved, total } => {
                    self.sync_status =
                        format!("Synchronisation terminée — {}/{} animes", saved, total);
                    self.reload_from_db();
                }
                SyncOutcome::Failure(msg) => {
                    self.sync_status = format!("Erreur — {}", msg);
                }
            }
        }
    }

    fn reload_from_db(&mut self) {
        let db = self.db.clone();
        let (loaded_animes, notes, downloaded, watched, tags, us_rows, va_rows) =
            self.runtime.block_on(async move {
                let guard = db.lock().await;
                (
                    guard.load_animes().unwrap_or_default(),
                    guard.load_user_notes().unwrap_or_default(),
                    guard.downloaded_anime_ids().unwrap_or_default(),
                    guard.load_watched().unwrap_or_default(),
                    guard.load_tags().unwrap_or_default(),
                    guard.load_us_animes().unwrap_or_default(),
                    guard.load_va_animes().unwrap_or_default(),
                )
            });

        let prev_expanded: HashMap<u64, bool> = self
            .animes
            .iter()
            .map(|a| (a.anime.id.to_bits(), a.expanded))
            .collect();

        let mut merged: Vec<AnimeDisplay> = loaded_animes
            .into_iter()
            .map(|anime| {
                let id_bits = anime.id.to_bits();
                let mut display = AnimeDisplay::new(anime);
                if let Some(&expanded) = prev_expanded.get(&id_bits) {
                    display.expanded = expanded;
                }
                if let Some(n) = notes.get(&id_bits) {
                    display.user_rating = n.rating;
                    display.user_comment = n.comment.clone().unwrap_or_default();
                    display.user_status = n.status.as_deref().and_then(UserStatus::from_db_key);
                }
                display.is_downloaded = downloaded.contains(&id_bits);
                if let Some(set) = watched.get(&id_bits) {
                    display.watched_eps = set.clone();
                }
                if let Some(t) = tags.get(&id_bits) {
                    display.user_tags = t.clone();
                }
                display
            })
            .collect();
        for row in &us_rows {
            if let Ok(series) =
                serde_json::from_str::<uniquestream::BrowseSeries>(&row.json_data)
            {
                let mut d = AnimeDisplay::from_us(&series);
                let id_bits = d.anime.id.to_bits();
                if let Some(&expanded) = prev_expanded.get(&id_bits) {
                    d.expanded = expanded;
                }
                if let Some(n) = notes.get(&id_bits) {
                    d.user_rating = n.rating;
                    d.user_comment = n.comment.clone().unwrap_or_default();
                    d.user_status = n.status.as_deref().and_then(UserStatus::from_db_key);
                }
                d.is_downloaded = downloaded.contains(&id_bits);
                if let Some(set) = watched.get(&id_bits) {
                    d.watched_eps = set.clone();
                }
                if let Some(t) = tags.get(&id_bits) {
                    d.user_tags = t.clone();
                }
                merged.push(d);
            }
        }
        for row in &va_rows {
            if let Ok(series) = serde_json::from_str::<voiranime::VaSeries>(&row.json_data) {
                let mut d = AnimeDisplay::from_va(&series);
                let id_bits = d.anime.id.to_bits();
                if let Some(&expanded) = prev_expanded.get(&id_bits) {
                    d.expanded = expanded;
                }
                if let Some(n) = notes.get(&id_bits) {
                    d.user_rating = n.rating;
                    d.user_comment = n.comment.clone().unwrap_or_default();
                    d.user_status = n.status.as_deref().and_then(UserStatus::from_db_key);
                }
                d.is_downloaded = downloaded.contains(&id_bits);
                if let Some(set) = watched.get(&id_bits) {
                    d.watched_eps = set.clone();
                }
                if let Some(t) = tags.get(&id_bits) {
                    d.user_tags = t.clone();
                }
                if let Some(eps_json) = &row.episodes_json {
                    if let Ok(cached) =
                        serde_json::from_str::<voiranime::VaCachedEpisodes>(eps_json)
                    {
                        apply_va_cached_to_anime(
                            &mut d,
                            &cached,
                            &self.va_episode_urls,
                            &self.va_episode_sources,
                        );
                    }
                }
                merged.push(d);
            }
        }
        merged.sort_by(|a, b| a.anime.title.to_lowercase().cmp(&b.anime.title.to_lowercase()));
        self.animes = merged;

        self.rebuild_themes_cache();
        self.rebuild_user_tags_cache();
        self.filter_animes();
    }

    fn queue_image_fetch(&self, url: &str, ctx: &egui::Context) {
        if url.is_empty() || !url.starts_with("http") {
            return;
        }
        {
            let mut set = self.image_fetching.lock().unwrap();
            if set.contains(url) {
                return;
            }
            set.insert(url.to_string());
        }
        let db = self.db.clone();
        let url_owned = url.to_string();
        let referer = if url_owned.contains("voir-anime.to") {
            Some("https://voir-anime.to/".to_string())
        } else if url_owned.contains("uniquestream.net") {
            Some("https://anime.uniquestream.net/".to_string())
        } else {
            None
        };
        let in_flight = self.image_fetching.clone();
        let ctx_clone = ctx.clone();
        let load_tx = self.image_load_tx.clone();
        self.runtime.spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36")
                .build();
            if let Ok(client) = client {
                let mut req = client.get(&url_owned);
                if let Some(r) = referer.as_ref() {
                    req = req.header("Referer", r);
                }
                if let Ok(resp) = req.send().await {
                    if resp.status().is_success() {
                        if let Ok(bytes) = resp.bytes().await {
                            let bytes_vec = bytes.to_vec();
                            let bytes_for_decode = bytes_vec.clone();
                            let decoded = tokio::task::spawn_blocking(move || {
                                image::load_from_memory(&bytes_for_decode).map(|img| {
                                    let rgba = img.to_rgba8();
                                    let (w, h) = rgba.dimensions();
                                    (w, h)
                                })
                            })
                            .await;
                            if let Ok(Ok((w, h))) = decoded {
                                let guard = db.lock().await;
                                let _ = guard.save_image(&url_owned, &bytes_vec, w, h);
                                drop(guard);
                                let _ = load_tx.send((url_owned.clone(), Some(bytes_vec)));
                                ctx_clone.request_repaint();
                            }
                        }
                    }
                }
            }
            in_flight.lock().unwrap().remove(&url_owned);
        });
    }

    fn load_image_from_db(
        &mut self,
        url: &str,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureHandle> {
        if url.is_empty() {
            return None;
        }
        if let Some(tex) = self.images.get(url) {
            return Some(tex.clone());
        }
        if self.image_missing.contains(url) {
            return None;
        }
        if self.image_loading.contains(url) {
            return None;
        }
        if self.image_fetching.lock().unwrap().contains(url) {
            return None;
        }
        self.image_loading.insert(url.to_string());
        let db = self.db.clone();
        let url_owned = url.to_string();
        let tx = self.image_load_tx.clone();
        self.runtime.spawn(async move {
            let bytes = {
                let guard = db.lock().await;
                guard.get_image(&url_owned).ok().flatten()
            };
            let _ = tx.send((url_owned, bytes));
        });
        None
    }

    fn drain_image_loads(&mut self, ctx: &egui::Context) {
        let mut budget = 64;
        while budget > 0 {
            let Ok((url, bytes_opt)) = self.image_load_rx.try_recv() else {
                break;
            };
            budget -= 1;
            self.image_loading.remove(&url);
            match bytes_opt {
                Some(bytes) => {
                    let decoded = image::load_from_memory(&bytes);
                    match decoded {
                        Ok(img) => {
                            let rgba = img.to_rgba8();
                            let (w, h) = rgba.dimensions();
                            let pixels = rgba.into_raw();
                            let color_image = ColorImage::from_rgba_unmultiplied(
                                [w as usize, h as usize],
                                &pixels,
                            );
                            let texture = ctx.load_texture(
                                &url,
                                color_image,
                                egui::TextureOptions::default(),
                            );
                            self.images.insert(url, texture);
                        }
                        Err(_) => {
                            self.image_missing.insert(url);
                        }
                    }
                }
                None => {
                    self.queue_image_fetch(&url, ctx);
                }
            }
        }
    }

    fn levenshtein_distance(s1: &str, s2: &str) -> usize {
        let len1 = s1.chars().count();
        let len2 = s2.chars().count();

        let mut matrix = vec![vec![0; len2 + 1]; len1 + 1];

        for i in 0..=len1 {
            matrix[i][0] = i;
        }
        for j in 0..=len2 {
            matrix[0][j] = j;
        }

        let s1_chars: Vec<char> = s1.chars().collect();
        let s2_chars: Vec<char> = s2.chars().collect();

        for i in 1..=len1 {
            for j in 1..=len2 {
                let cost = if s1_chars[i - 1] == s2_chars[j - 1] {
                    0
                } else {
                    1
                };
                matrix[i][j] = (matrix[i - 1][j] + 1)
                    .min(matrix[i][j - 1] + 1)
                    .min(matrix[i - 1][j - 1] + cost);
            }
        }

        matrix[len1][len2]
    }

    fn matches_search(anime: &AnimeDisplay, query_lower: &str) -> bool {
        if query_lower.is_empty() {
            return true;
        }

        if anime.anime.title.to_lowercase().contains(query_lower)
            || anime.anime.title_o.to_lowercase().contains(query_lower)
        {
            return true;
        }

        if let Some(ref ja_jp) = anime.anime.titles.ja_jp {
            if ja_jp.to_lowercase().contains(query_lower) {
                return true;
            }
        }

        if anime
            .anime
            .description
            .to_lowercase()
            .contains(query_lower)
        {
            return true;
        }

        for theme in &anime.anime.themes {
            if theme.to_lowercase().contains(query_lower) {
                return true;
            }
        }

        if !anime.user_comment.is_empty()
            && anime.user_comment.to_lowercase().contains(query_lower)
        {
            return true;
        }

        let threshold = (query_lower.chars().count() as f32 * 0.4) as usize;
        if threshold == 0 {
            return false;
        }
        let title_lower = anime.anime.title.to_lowercase();
        Self::levenshtein_distance(query_lower, &title_lower) <= threshold
    }

    fn matches_themes(&self, anime: &AnimeDisplay) -> bool {
        if self.selected_themes.is_empty() {
            return true;
        }
        let anime_themes: std::collections::HashSet<&str> = anime
            .anime
            .themes
            .iter()
            .map(|s| s.as_str())
            .collect();
        match self.theme_filter_mode {
            ThemeFilterMode::Any => self
                .selected_themes
                .iter()
                .any(|t| anime_themes.contains(t.as_str())),
            ThemeFilterMode::All => self
                .selected_themes
                .iter()
                .all(|t| anime_themes.contains(t.as_str())),
        }
    }

    fn filter_animes(&mut self) {
        let query_lower = self.search_query.to_lowercase();
        let min_rating = self.min_user_rating;
        let only_dl = self.only_downloaded;
        let hide_nsfw = self.hide_nsfw;
        let lang_filter = self.lang_filter.clone();
        let statuses = self.selected_statuses.clone();
        let req_tags = self.selected_user_tags.clone();
        let theme_check: Vec<(usize, bool)> = self
            .animes
            .iter()
            .enumerate()
            .map(|(i, a)| (i, self.matches_themes(a)))
            .collect();
        let theme_ok: std::collections::HashMap<usize, bool> = theme_check.into_iter().collect();

        let mut indices: Vec<usize> = self
            .animes
            .iter()
            .enumerate()
            .filter(|(idx, anime)| {
                let lang_match = match lang_filter {
                    LangFilter::All => true,
                    LangFilter::VO => anime.has_vo && !anime.has_vf,
                    LangFilter::VF => anime.has_vf && !anime.has_vo,
                    LangFilter::Both => anime.has_vo && anime.has_vf,
                };
                let rating_ok = min_rating <= 0.0
                    || anime.user_rating.map(|r| r >= min_rating).unwrap_or(false);
                let dl_ok = !only_dl || anime.is_downloaded;
                let nsfw_ok = !hide_nsfw || !anime.anime.nsfw;
                let theme_ok = *theme_ok.get(idx).unwrap_or(&true);
                let status_ok = statuses.is_empty()
                    || anime.user_status.map(|s| statuses.contains(&s)).unwrap_or(false);
                let tag_ok = req_tags.is_empty()
                    || req_tags.iter().all(|t| anime.user_tags.contains(t));
                lang_match
                    && rating_ok
                    && dl_ok
                    && nsfw_ok
                    && theme_ok
                    && status_ok
                    && tag_ok
                    && Self::matches_search(anime, &query_lower)
            })
            .map(|(idx, _)| idx)
            .collect();

        let sort_mode = self.sort_mode;
        let desc = self.sort_descending;
        indices.sort_by(|&a, &b| {
            let aa = &self.animes[a];
            let bb = &self.animes[b];
            let ord = match sort_mode {
                SortMode::TitleAlpha => aa.anime.title.to_lowercase().cmp(&bb.anime.title.to_lowercase()),
                SortMode::StartDate => aa.anime.start_date.cmp(&bb.anime.start_date),
                SortMode::UserRating => {
                    let ax = aa.user_rating.unwrap_or(-1.0);
                    let bx = bb.user_rating.unwrap_or(-1.0);
                    ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal)
                }
                SortMode::SiteRating => {
                    let ax: f32 = aa.anime.note.parse().unwrap_or(-1.0);
                    let bx: f32 = bb.anime.note.parse().unwrap_or(-1.0);
                    ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal)
                }
                SortMode::EpisodesCount => aa.total_episodes().cmp(&bb.total_episodes()),
                SortMode::SeasonsCount => aa.anime.saisons.len().cmp(&bb.anime.saisons.len()),
            };
            if desc { ord.reverse() } else { ord }
        });

        self.filtered_indices = indices;
    }

    fn valid_lecteurs_for_host(
        anime: &Root2,
        season_idx: usize,
        ep_idx: usize,
        lang: &str,
        preferred_host: Option<&str>,
    ) -> Vec<u64> {
        let Some(saison) = anime.saisons.get(season_idx) else {
            return Vec::new();
        };
        let Some(episode) = saison.episodes.get(ep_idx) else {
            return Vec::new();
        };
        let names = if lang == "vf" {
            &episode.lang.vf.lecteurs
        } else {
            &episode.lang.vo.lecteurs
        };
        let valid: Vec<(u64, &str)> = names
            .iter()
            .enumerate()
            .filter(|(_, n)| n.as_str() != "hd" && n.as_str() != "TELECHARGEMENT UNIQUE")
            .map(|(i, n)| (i as u64, n.as_str()))
            .collect();

        let mut ordered = Vec::with_capacity(valid.len());
        if let Some(host) = preferred_host {
            let host_lower = host.to_lowercase();
            for (i, n) in &valid {
                if n.to_lowercase().contains(&host_lower) {
                    ordered.push(*i);
                }
            }
        }
        for (i, _) in &valid {
            if !ordered.contains(i) {
                ordered.push(*i);
            }
        }
        ordered
    }

    fn spawn_episode_download(&self, original: OriginalRequest) {
        let anime = original.anime.clone();
        let source = self
            .animes
            .iter()
            .find(|d| d.anime.id == anime.id)
            .map(|d| d.source)
            .unwrap_or(AnimeSource::Franime);
        let Some(saison) = anime.saisons.get(original.season_idx) else {
            return;
        };
        let Some(episode) = saison.episodes.get(original.ep_idx) else {
            return;
        };

        let anime_name = if anime.title_o.is_empty() {
            anime.title.clone()
        } else {
            anime.title_o.clone()
        };
        let season_name = saison.title.clone();
        let episode_name = episode.title.clone();
        let anime_id = anime.id as u64;
        let lang = original.lang;
        let dir_root = self
            .settings
            .effective_download_dir()
            .join(if lang == "vf" { "download_VF" } else { "download_VO" })
            .join(sanitize_path_segment(&anime_name))
            .join(sanitize_path_segment(&season_name));
        let file = build_filename(
            &self.settings.naming_format,
            &anime_name,
            original.season_idx,
            original.ep_idx,
            &episode_name,
            lang,
        );

        if let Err(e) = std::fs::create_dir_all(&dir_root) {
            eprintln!("Création de '{}' échouée: {}", dir_root.display(), e);
            return;
        }
        let output_path = dir_root.join(&file);

        if self.settings.skip_existing && output_path.exists() {
            applog::log_event(
                applog::LogSource::App,
                applog::LogLevel::Info,
                format!(
                    "Skip: fichier déjà présent {}",
                    output_path.display()
                ),
            );
            let db = self.db.clone();
            let path_str = output_path.to_string_lossy().into_owned();
            let s_idx = original.season_idx;
            let e_idx = original.ep_idx;
            let lang_str = lang.to_string();
            self.runtime.spawn(async move {
                let guard = db.lock().await;
                let _ = guard.record_download(
                    anime.id,
                    s_idx,
                    e_idx,
                    &lang_str,
                    &path_str,
                );
            });
            return;
        }

        let lecteurs = original.lecteurs_to_try.clone();
        if lecteurs.is_empty() {
            eprintln!(
                "Aucun lecteur disponible pour S{}E{} ({})",
                original.season_idx, original.ep_idx, lang
            );
            return;
        }

        let fetcher = self.fetcher.clone();
        let manager = self.manager.clone();
        let originals = self.task_originals.clone();
        let original_for_map = original.clone();
        let label = format!(
            "{} - S{}E{} ({})",
            anime_name,
            original.season_idx + 1,
            original.ep_idx + 1,
            lang.to_uppercase()
        );

        let lecteur_names: HashMap<u64, String> = {
            let names = if lang == "vf" {
                &saison.episodes[original.ep_idx].lang.vf.lecteurs
            } else {
                &saison.episodes[original.ep_idx].lang.vo.lecteurs
            };
            names
                .iter()
                .enumerate()
                .map(|(i, n)| (i as u64, n.clone()))
                .collect()
        };

        let us_ep_id: Option<String> = self
            .us_episode_ids
            .lock()
            .unwrap()
            .get(&(
                anime.id.to_bits(),
                original.season_idx,
                original.ep_idx,
            ))
            .cloned();
        let us_audio_locales: Vec<String> = self
            .us_audio_locales
            .lock()
            .unwrap()
            .get(&anime.id.to_bits())
            .cloned()
            .unwrap_or_default();
        let us_is_movie: bool = self
            .animes
            .iter()
            .find(|d| d.anime.id == anime.id)
            .and_then(|d| d.us_content_id.clone())
            .map(|cid| self.us_movies.lock().unwrap().contains(&cid))
            .unwrap_or(false);

        let va_ep_url: Option<String> = self
            .va_episode_urls
            .lock()
            .unwrap()
            .get(&(
                anime.id.to_bits(),
                original.season_idx,
                original.ep_idx,
            ))
            .cloned();
        let va_ep_sources: Vec<voiranime::VaSource> = self
            .va_episode_sources
            .lock()
            .unwrap()
            .get(&(
                anime.id.to_bits(),
                original.season_idx,
                original.ep_idx,
            ))
            .cloned()
            .unwrap_or_default();

        let consumet_url = self.settings.consumet_base_url.clone();
        let consumet_provider = self.settings.consumet_provider.clone();
        let consumet_enabled = self.settings.consumet_enabled;
        let consumet_auto_fallback = self.settings.consumet_auto_fallback;
        let anikuro_enabled = self.settings.anikuro_enabled;
        let anikuro_auto_fallback = self.settings.anikuro_auto_fallback;
        let anikuro_provider = self.settings.anikuro_provider.clone();
        let anikuro_prefer_dub = self.settings.anikuro_prefer_dub || lang == "vf";
        let (anime_name_for_consumet, alt_titles_for_consumet) =
            self.collect_consumet_query(&anime);
        let absolute_ep_number: usize = anime
            .saisons
            .iter()
            .take(original.season_idx)
            .map(|s| s.episodes.len())
            .sum::<usize>()
            + original.ep_idx
            + 1;

        self.runtime.spawn(async move {
            let task_id = manager.add_pending(output_path).await;
            originals
                .lock()
                .unwrap()
                .insert(task_id.clone(), original_for_map);

            if source == AnimeSource::Voiranime && va_ep_url.is_none() {
                manager
                    .mark_failed(
                        &task_id,
                        "voiranime: épisodes pas encore chargés (déplie l'anime d'abord)".to_string(),
                    )
                    .await;
                return;
            }
            if source == AnimeSource::Uniquestream && us_ep_id.is_none() {
                manager
                    .mark_failed(
                        &task_id,
                        "uniquestream: épisodes pas encore chargés (déplie l'anime d'abord)".to_string(),
                    )
                    .await;
                return;
            }

            if source == AnimeSource::Voiranime {
                let mut va_attempts: Vec<(String, String)> = Vec::new();
                if !va_ep_sources.is_empty() {
                    for lecteur_idx in &lecteurs {
                        let i = *lecteur_idx as usize;
                        let Some(src) = va_ep_sources.get(i) else {
                            continue;
                        };
                        let host_label = format!("voiranime:{}", src.host);
                        manager.set_task_host(&task_id, Some(host_label.clone())).await;
                        manager.add_attempted_lecteur(&task_id, *lecteur_idx).await;
                        applog::log_event(
                            applog::LogSource::App,
                            applog::LogLevel::Info,
                            format!(
                                "voiranime DL via {} → {}",
                                src.host,
                                &src.iframe[..src.iframe.len().min(120)]
                            ),
                        );
                        match manager
                            .extract_and_download(task_id.clone(), src.iframe.clone())
                            .await
                        {
                            Ok(()) => return,
                            Err(e) => {
                                applog::log_event(
                                    applog::LogSource::App,
                                    applog::LogLevel::Warn,
                                    format!("voiranime {} KO: {}", src.host, e),
                                );
                                va_attempts.push((src.host.clone(), format!("{}", e)));
                                continue;
                            }
                        }
                    }
                    let summary = if va_attempts.is_empty() {
                        "voiranime: aucune source utilisable".to_string()
                    } else {
                        let parts: Vec<String> = va_attempts
                            .iter()
                            .map(|(h, e)| format!("{}: {}", h, e))
                            .collect();
                        format!("voiranime: tous lecteurs KO — {}", parts.join(" · "))
                    };
                    manager.mark_failed(&task_id, summary).await;
                    return;
                } else if let Some(ep_url) = va_ep_url.clone() {
                    applog::log_event(
                        applog::LogSource::App,
                        applog::LogLevel::Info,
                        format!("voiranime DL (fallback iframe) ep_url={}", ep_url),
                    );
                    manager
                        .set_task_host(&task_id, Some("voiranime".to_string()))
                        .await;
                    match voiranime::VaClient::new() {
                        Ok(va_client) => match va_client.episode_iframe(&ep_url).await {
                            Ok(iframe) => {
                                manager
                                    .set_task_host(
                                        &task_id,
                                        Some(format!(
                                            "voiranime via {}",
                                            host_from_url(&iframe)
                                        )),
                                    )
                                    .await;
                                match manager
                                    .extract_and_download(task_id.clone(), iframe)
                                    .await
                                {
                                    Ok(()) => return,
                                    Err(e) => {
                                        manager
                                            .mark_failed(
                                                &task_id,
                                                format!("voiranime DL: {}", e),
                                            )
                                            .await;
                                        return;
                                    }
                                }
                            }
                            Err(e) => {
                                manager
                                    .mark_failed(
                                        &task_id,
                                        format!("voiranime iframe: {}", e),
                                    )
                                    .await;
                                return;
                            }
                        },
                        Err(e) => {
                            manager
                                .mark_failed(
                                    &task_id,
                                    format!("voiranime client: {}", e),
                                )
                                .await;
                            return;
                        }
                    }
                }
            }

            if let Some(ep_cid) = us_ep_id {
                let prefer_dub = lang == "vf";
                let locale = uniquestream::pick_audio_locale(
                    &Some(us_audio_locales.clone()),
                    prefer_dub,
                );
                applog::log_event(
                    applog::LogSource::App,
                    applog::LogLevel::Info,
                    format!(
                        "uniquestream DL ep_id={} locale={}",
                        ep_cid, locale
                    ),
                );
                manager
                    .set_task_host(&task_id, Some(format!("uniquestream:{}", locale)))
                    .await;
                match uniquestream::UsClient::new() {
                    Ok(client) => match if us_is_movie {
                        client.movie_media_hls(&ep_cid, &locale).await
                    } else {
                        client.episode_media_hls(&ep_cid, &locale).await
                    } {
                        Ok(url) => {
                            applog::log_event(
                                applog::LogSource::App,
                                applog::LogLevel::Info,
                                format!(
                                    "uniquestream HLS: {}",
                                    &url[..url.len().min(120)]
                                ),
                            );
                            match manager.download_direct(task_id.clone(), url).await {
                                Ok(()) => return,
                                Err(e) => {
                                    manager
                                        .mark_failed(
                                            &task_id,
                                            format!("uniquestream DL: {}", e),
                                        )
                                        .await;
                                    return;
                                }
                            }
                        }
                        Err(e) => {
                            manager
                                .mark_failed(
                                    &task_id,
                                    format!("uniquestream media: {}", e),
                                )
                                .await;
                            return;
                        }
                    },
                    Err(e) => {
                        manager
                            .mark_failed(
                                &task_id,
                                format!("uniquestream client: {}", e),
                            )
                            .await;
                        return;
                    }
                }
            }

            let mut attempts: Vec<(String, String)> = Vec::new();
            for lecteur in &lecteurs {
                let host_name = lecteur_names
                    .get(lecteur)
                    .cloned()
                    .unwrap_or_else(|| format!("#{}", lecteur));

                manager.set_task_host(&task_id, Some(host_name.clone())).await;
                manager.add_attempted_lecteur(&task_id, *lecteur).await;

                let iframe_url = match fetcher
                    .fetch_video_url(
                        anime_id,
                        original.season_idx as u64,
                        original.ep_idx as u64,
                        lang,
                        *lecteur,
                    )
                    .await
                {
                    Ok(u) => u,
                    Err(e) => {
                        let err = format!("API: {}", e);
                        eprintln!("{} — {}: {}", label, host_name, err);
                        attempts.push((host_name, err));
                        continue;
                    }
                };

                match manager
                    .extract_and_download(task_id.clone(), iframe_url)
                    .await
                {
                    Ok(()) => return,
                    Err(e) => {
                        let err = format!("{}", e);
                        eprintln!("{} — {}: {}", label, host_name, err);
                        attempts.push((host_name, err));
                        continue;
                    }
                }
            }

            let summary = if attempts.is_empty() {
                "Aucune tentative".to_string()
            } else {
                let parts: Vec<String> = attempts
                    .iter()
                    .map(|(host, err)| format!("{}: {}", host, err))
                    .collect();
                format!("Tous les lecteurs ont échoué — {}", parts.join(" · "))
            };

            if anikuro_enabled && anikuro_auto_fallback {
                applog::log_event(
                    applog::LogSource::App,
                    applog::LogLevel::Info,
                    format!(
                        "Fallback Anikuro auto ({}): {} ep {} ({})",
                        anikuro_provider,
                        anime_name_for_consumet,
                        absolute_ep_number,
                        if anikuro_prefer_dub { "dub" } else { "sub" },
                    ),
                );
                manager
                    .set_task_host(&task_id, Some(format!("anikuro:{}", anikuro_provider)))
                    .await;
                match try_anikuro(
                    &anikuro_provider,
                    &anime_name_for_consumet,
                    &alt_titles_for_consumet,
                    absolute_ep_number,
                    anikuro_prefer_dub,
                )
                .await
                {
                    Ok(stream_url) => {
                        applog::log_event(
                            applog::LogSource::App,
                            applog::LogLevel::Info,
                            format!(
                                "Anikuro stream URL: {}",
                                &stream_url[..stream_url.len().min(120)]
                            ),
                        );
                        match manager.download_direct(task_id.clone(), stream_url).await {
                            Ok(()) => return,
                            Err(e) => {
                                applog::log_event(
                                    applog::LogSource::App,
                                    applog::LogLevel::Warn,
                                    format!("Anikuro download échoué: {}", e),
                                );
                            }
                        }
                    }
                    Err(e) => {
                        applog::log_event(
                            applog::LogSource::App,
                            applog::LogLevel::Warn,
                            format!("Anikuro search/sources échoué: {}", e),
                        );
                    }
                }
            }

            if consumet_auto_fallback && consumet_enabled && !consumet_url.is_empty() {
                applog::log_event(
                    applog::LogSource::App,
                    applog::LogLevel::Info,
                    format!(
                        "Fallback Consumet auto ({}): {} ep {}",
                        consumet_provider,
                        anime_name_for_consumet,
                        absolute_ep_number,
                    ),
                );
                manager.set_task_host(&task_id, Some(format!("consumet:{}", consumet_provider))).await;
                match try_consumet(
                    &consumet_url,
                    &consumet_provider,
                    &anime_name_for_consumet,
                    &alt_titles_for_consumet,
                    absolute_ep_number,
                )
                .await
                {
                    Ok(stream_url) => {
                        applog::log_event(
                            applog::LogSource::App,
                            applog::LogLevel::Info,
                            format!("Consumet stream URL: {}", &stream_url[..stream_url.len().min(120)]),
                        );
                        match manager.download_direct(task_id.clone(), stream_url).await {
                            Ok(()) => return,
                            Err(e) => {
                                applog::log_event(
                                    applog::LogSource::App,
                                    applog::LogLevel::Error,
                                    format!("Consumet download échoué: {}", e),
                                );
                                manager
                                    .mark_failed(
                                        &task_id,
                                        format!("{} · consumet: {}", summary, e),
                                    )
                                    .await;
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        applog::log_event(
                            applog::LogSource::App,
                            applog::LogLevel::Warn,
                            format!("Consumet search/info échoué: {}", e),
                        );
                        manager
                            .mark_failed(
                                &task_id,
                                format!("{} · consumet: {}", summary, e),
                            )
                            .await;
                        return;
                    }
                }
            }

            manager.mark_failed(&task_id, summary).await;
        });
    }

    fn collect_consumet_query(&self, anime: &Root2) -> (String, Vec<String>) {
        let mut alts = Vec::new();
        if !anime.title_o.is_empty() && anime.title_o != anime.title {
            alts.push(anime.title_o.clone());
        }
        if let Some(ref ja) = anime.titles.ja_jp {
            if !ja.is_empty() {
                alts.push(ja.clone());
            }
        }
        (anime.title.clone(), alts)
    }

    fn enqueue_download(
        &self,
        anime: &Root2,
        season_idx: usize,
        ep_idx: usize,
        lang: &'static str,
        preferred_host: Option<&str>,
    ) {
        let source = self
            .animes
            .iter()
            .find(|d| d.anime.id == anime.id)
            .map(|d| d.source)
            .unwrap_or(AnimeSource::Franime);

        let lecteurs_to_try = if source == AnimeSource::Franime {
            let fallback = self.settings.preferred_lecteur_host.clone();
            let effective_host: Option<&str> = preferred_host.or_else(|| {
                if fallback.is_empty() {
                    None
                } else {
                    Some(fallback.as_str())
                }
            });
            Self::valid_lecteurs_for_host(anime, season_idx, ep_idx, lang, effective_host)
        } else if source == AnimeSource::Voiranime {
            let sources = self
                .va_episode_sources
                .lock()
                .unwrap()
                .get(&(anime.id.to_bits(), season_idx, ep_idx))
                .cloned()
                .unwrap_or_default();
            if sources.is_empty() {
                vec![0u64]
            } else {
                let mut ordered: Vec<u64> = Vec::new();
                if let Some(host) = preferred_host {
                    let host_lower = host.to_lowercase();
                    for (i, s) in sources.iter().enumerate() {
                        if s.host.to_lowercase() == host_lower
                            || s.host.to_lowercase().contains(&host_lower)
                        {
                            ordered.push(i as u64);
                        }
                    }
                }
                for i in 0..sources.len() {
                    if !ordered.contains(&(i as u64)) {
                        ordered.push(i as u64);
                    }
                }
                ordered
            }
        } else {
            vec![0u64]
        };
        if lecteurs_to_try.is_empty() {
            return;
        }
        self.spawn_episode_download(OriginalRequest {
            anime: anime.clone(),
            season_idx,
            ep_idx,
            lang,
            lecteurs_to_try,
        });
    }

    fn enqueue_anime(
        &self,
        anime: &Root2,
        lang: &'static str,
        preferred_host: Option<&str>,
        only_season: Option<usize>,
    ) {
        for (s_idx, saison) in anime.saisons.iter().enumerate() {
            if let Some(target) = only_season {
                if s_idx != target {
                    continue;
                }
            }
            for (e_idx, _) in saison.episodes.iter().enumerate() {
                self.enqueue_download(anime, s_idx, e_idx, lang, preferred_host);
            }
        }
    }

    fn host_options_for(&self, anime: &Root2) -> Vec<String> {
        let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for saison in &anime.saisons {
            for episode in &saison.episodes {
                for n in episode
                    .lang
                    .vo
                    .lecteurs
                    .iter()
                    .chain(episode.lang.vf.lecteurs.iter())
                {
                    let s = n.as_str();
                    if s != "hd" && s != "TELECHARGEMENT UNIQUE" {
                        set.insert(s.to_string());
                    }
                }
            }
        }
        set.into_iter().collect()
    }

    fn retry_task(&self, id: &str) {
        let original = self.task_originals.lock().unwrap().get(id).cloned();
        let Some(original) = original else {
            eprintln!("Pas de request originale pour la tâche {}", id);
            return;
        };
        let manager = self.manager.clone();
        let id_owned = id.to_string();
        self.runtime.spawn(async move {
            manager.forget(&id_owned).await;
        });
        self.task_originals.lock().unwrap().remove(id);
        self.spawn_episode_download(original);
    }

    fn cancel_task(&self, id: &str) {
        let manager = self.manager.clone();
        let id_owned = id.to_string();
        self.runtime.spawn(async move {
            manager.cancel(&id_owned).await;
        });
    }

    fn retry_via_anikuro(&self, id: &str) {
        let original = self.task_originals.lock().unwrap().get(id).cloned();
        let Some(original) = original else {
            return;
        };
        if !self.settings.anikuro_enabled {
            return;
        }
        let provider = self.settings.anikuro_provider.clone();
        let prefer_dub = self.settings.anikuro_prefer_dub || original.lang == "vf";
        let (title, alts) = self.collect_consumet_query(&original.anime);
        let absolute_ep_number: usize = original
            .anime
            .saisons
            .iter()
            .take(original.season_idx)
            .map(|s| s.episodes.len())
            .sum::<usize>()
            + original.ep_idx
            + 1;
        let manager = self.manager.clone();
        let id_owned = id.to_string();
        self.runtime.spawn(async move {
            manager
                .set_task_host(&id_owned, Some(format!("anikuro:{}", provider)))
                .await;
            applog::log_event(
                applog::LogSource::App,
                applog::LogLevel::Info,
                format!(
                    "Anikuro ({}): {} ep {} ({})",
                    provider,
                    title,
                    absolute_ep_number,
                    if prefer_dub { "dub" } else { "sub" }
                ),
            );
            match try_anikuro(&provider, &title, &alts, absolute_ep_number, prefer_dub).await {
                Ok(stream_url) => {
                    if let Err(e) = manager.download_direct(id_owned.clone(), stream_url).await {
                        manager
                            .mark_failed(&id_owned, format!("anikuro: {}", e))
                            .await;
                    }
                }
                Err(e) => {
                    manager
                        .mark_failed(&id_owned, format!("anikuro: {}", e))
                        .await;
                }
            }
        });
    }

    fn retry_via_consumet(&self, id: &str) {
        let original = self.task_originals.lock().unwrap().get(id).cloned();
        let Some(original) = original else {
            return;
        };
        if !self.settings.consumet_enabled || self.settings.consumet_base_url.is_empty() {
            applog::log_event(
                applog::LogSource::App,
                applog::LogLevel::Warn,
                "Consumet désactivé ou URL vide",
            );
            return;
        }
        let consumet_url = self.settings.consumet_base_url.clone();
        let consumet_provider = self.settings.consumet_provider.clone();
        let (title, alts) = self.collect_consumet_query(&original.anime);
        let absolute_ep_number: usize = original
            .anime
            .saisons
            .iter()
            .take(original.season_idx)
            .map(|s| s.episodes.len())
            .sum::<usize>()
            + original.ep_idx
            + 1;
        let manager = self.manager.clone();
        let id_owned = id.to_string();
        self.runtime.spawn(async move {
            manager
                .set_task_host(&id_owned, Some(format!("consumet:{}", consumet_provider)))
                .await;
            applog::log_event(
                applog::LogSource::App,
                applog::LogLevel::Info,
                format!(
                    "Source alternative Consumet ({}): {} ep {}",
                    consumet_provider, title, absolute_ep_number
                ),
            );
            match try_consumet(
                &consumet_url,
                &consumet_provider,
                &title,
                &alts,
                absolute_ep_number,
            )
            .await
            {
                Ok(stream_url) => {
                    if let Err(e) = manager.download_direct(id_owned.clone(), stream_url).await {
                        manager
                            .mark_failed(&id_owned, format!("consumet: {}", e))
                            .await;
                    }
                }
                Err(e) => {
                    manager
                        .mark_failed(&id_owned, format!("consumet: {}", e))
                        .await;
                }
            }
        });
    }

    fn skip_to_next_host(&self, id: &str) {
        let original = self.task_originals.lock().unwrap().get(id).cloned();
        let Some(original) = original else {
            return;
        };
        let attempted: Vec<u64> = self
            .task_view
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.attempted_lecteurs.clone())
            .unwrap_or_default();
        let remaining: Vec<u64> = original
            .lecteurs_to_try
            .iter()
            .copied()
            .filter(|l| !attempted.contains(l))
            .collect();
        if remaining.is_empty() {
            applog::log_event(
                applog::LogSource::App,
                applog::LogLevel::Warn,
                "Plus de host à essayer pour cette tâche",
            );
            return;
        }
        let manager = self.manager.clone();
        let id_owned = id.to_string();
        self.runtime.spawn(async move {
            manager.cancel(&id_owned).await;
            manager.forget(&id_owned).await;
        });
        self.task_originals.lock().unwrap().remove(id);
        self.spawn_episode_download(OriginalRequest {
            anime: original.anime,
            season_idx: original.season_idx,
            ep_idx: original.ep_idx,
            lang: original.lang,
            lecteurs_to_try: remaining,
        });
    }

    fn forget_task(&self, id: &str) {
        let manager = self.manager.clone();
        let id_owned = id.to_string();
        self.runtime.spawn(async move {
            manager.forget(&id_owned).await;
        });
        self.task_originals.lock().unwrap().remove(id);
    }

    fn clear_finished(&self) {
        let view = self.task_view.lock().unwrap().clone();
        for task in view {
            if matches!(
                task.status,
                DlStatus::Completed | DlStatus::Failed(_) | DlStatus::Cancelled
            ) {
                self.forget_task(&task.id);
            }
        }
    }

    fn render_downloads_panel(&self, ui: &mut egui::Ui) {
        let tasks: Vec<DownloadTask> = self.task_view.lock().unwrap().clone();

        ui.horizontal(|ui| {
            ui.heading(
                RichText::new("Téléchargements")
                    .size(18.0)
                    .color(Color32::from_rgb(189, 147, 249))
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let btn = egui::Button::new(
                    RichText::new("Nettoyer")
                        .size(11.0)
                        .color(Color32::WHITE),
                )
                .fill(Color32::from_rgb(68, 71, 90))
                .corner_radius(5.0);
                if ui.add(btn).clicked() {
                    self.clear_finished();
                }
            });
        });

        let total = tasks.len();
        let active = tasks
            .iter()
            .filter(|t| {
                matches!(
                    t.status,
                    DlStatus::Queued | DlStatus::Extracting | DlStatus::Downloading(_)
                )
            })
            .count();
        let done = tasks
            .iter()
            .filter(|t| matches!(t.status, DlStatus::Completed))
            .count();
        let failed = tasks
            .iter()
            .filter(|t| matches!(t.status, DlStatus::Failed(_)))
            .count();

        ui.label(
            RichText::new(format!(
                "{} total  ·  {} actifs  ·  {} OK  ·  {} échec",
                total, active, done, failed
            ))
            .size(11.0)
            .color(Color32::from_rgb(150, 150, 160)),
        );

        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                if tasks.is_empty() {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("Aucun téléchargement")
                                .size(12.0)
                                .color(Color32::from_rgb(120, 120, 130))
                                .italics(),
                        );
                    });
                    return;
                }

                for task in &tasks {
                    self.render_task_row(ui, task);
                    ui.add_space(4.0);
                }
            });
    }

    fn render_task_row(&self, ui: &mut egui::Ui, task: &DownloadTask) {
        let original = self.task_originals.lock().unwrap().get(&task.id).cloned();

        let (title_line, subtitle_line) = if let Some(orig) = &original {
            let anime_title = if orig.anime.title.is_empty() {
                orig.anime.title_o.clone()
            } else {
                orig.anime.title.clone()
            };
            let saison_label = orig
                .anime
                .saisons
                .get(orig.season_idx)
                .map(|s| s.title.clone())
                .unwrap_or_else(|| format!("Saison {}", orig.season_idx + 1));
            let ep_label = orig
                .anime
                .saisons
                .get(orig.season_idx)
                .and_then(|s| s.episodes.get(orig.ep_idx))
                .map(|e| e.title.clone())
                .unwrap_or_else(|| format!("Épisode {}", orig.ep_idx + 1));
            let lang_label = orig.lang.to_uppercase();
            let host_suffix = match &task.host {
                Some(h) => format!("  ·  via {}", h),
                None => String::new(),
            };
            (
                anime_title,
                format!(
                    "{} · Ép. {} · {} · {}{}",
                    saison_label,
                    orig.ep_idx + 1,
                    ep_label,
                    lang_label,
                    host_suffix,
                ),
            )
        } else {
            let file_name = task
                .output_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_string();
            (file_name, String::new())
        };

        let (status_text, status_color, progress, status_detail) = match &task.status {
            DlStatus::Queued => (
                "En file".to_string(),
                Color32::from_rgb(150, 150, 160),
                None,
                None,
            ),
            DlStatus::Extracting => (
                "Extraction de l'URL vidéo…".to_string(),
                Color32::from_rgb(241, 250, 140),
                None,
                None,
            ),
            DlStatus::Downloading(p) => {
                let speed = format_speed(p.speed_bytes_per_sec);
                let eta = format_eta(p.eta_seconds);
                let res_prefix = p
                    .resolution
                    .as_ref()
                    .map(|r| format!("{}  ·  ", r))
                    .unwrap_or_default();
                let text = if p.total > 0 {
                    format!(
                        "{}{:.1}%  ·  {}  ·  ETA {}",
                        res_prefix, p.percentage, speed, eta
                    )
                } else {
                    format!(
                        "{}{}s traités  ·  {}",
                        res_prefix, p.downloaded, speed
                    )
                };
                let detail = if p.total > 0 {
                    let bytes = format_bytes(p.downloaded);
                    let total = format_bytes(p.total);
                    Some(format!("{} / {}", bytes, total))
                } else {
                    None
                };
                (
                    text,
                    Color32::from_rgb(139, 233, 253),
                    if p.total > 0 {
                        Some(p.percentage / 100.0)
                    } else {
                        None
                    },
                    detail,
                )
            }
            DlStatus::Completed => (
                "Terminé".to_string(),
                Color32::from_rgb(80, 250, 123),
                Some(1.0),
                None,
            ),
            DlStatus::Failed(msg) => (
                "Échec".to_string(),
                Color32::from_rgb(255, 85, 85),
                None,
                Some(msg.clone()),
            ),
            DlStatus::Cancelled => (
                "Annulé".to_string(),
                Color32::from_rgb(180, 120, 120),
                None,
                None,
            ),
        };

        egui::Frame::NONE
            .fill(Color32::from_rgb(50, 52, 64))
            .corner_radius(6.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(&title_line)
                            .size(12.0)
                            .color(Color32::from_rgb(220, 220, 230))
                            .strong(),
                    );
                    if !subtitle_line.is_empty() {
                        ui.label(
                            RichText::new(&subtitle_line)
                                .size(10.0)
                                .color(Color32::from_rgb(150, 150, 160)),
                        );
                    }

                    ui.add_space(4.0);

                    if let Some(frac) = progress {
                        ui.add(
                            egui::ProgressBar::new(frac)
                                .desired_height(8.0)
                                .fill(Color32::from_rgb(139, 233, 253)),
                        );
                    }

                    ui.label(RichText::new(status_text).size(11.0).color(status_color));
                    if let Some(detail) = status_detail {
                        ui.label(
                            RichText::new(detail)
                                .size(10.0)
                                .color(Color32::from_rgb(150, 150, 160)),
                        );
                    }

                    ui.horizontal(|ui| {
                        let active = matches!(
                            task.status,
                            DlStatus::Queued | DlStatus::Extracting | DlStatus::Downloading(_)
                        );
                        let failed = matches!(task.status, DlStatus::Failed(_));
                        let finished = matches!(
                            task.status,
                            DlStatus::Completed | DlStatus::Failed(_) | DlStatus::Cancelled
                        );

                        if failed {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Réessayer").size(10.0).color(Color32::WHITE),
                                    )
                                    .fill(Color32::from_rgb(189, 147, 249))
                                    .corner_radius(4.0),
                                )
                                .clicked()
                            {
                                self.retry_task(&task.id);
                            }
                            if self.settings.anikuro_enabled
                                && ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new("Anikuro")
                                                .size(10.0)
                                                .color(Color32::BLACK),
                                        )
                                        .fill(Color32::from_rgb(139, 233, 253))
                                        .corner_radius(4.0),
                                    )
                                    .clicked()
                            {
                                self.retry_via_anikuro(&task.id);
                            }
                            if self.settings.consumet_enabled
                                && !self.settings.consumet_base_url.is_empty()
                                && ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new("Consumet")
                                                .size(10.0)
                                                .color(Color32::WHITE),
                                        )
                                        .fill(Color32::from_rgb(80, 250, 123))
                                        .corner_radius(4.0),
                                    )
                                    .clicked()
                            {
                                self.retry_via_consumet(&task.id);
                            }
                        }
                        if active {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Annuler")
                                            .size(10.0)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(Color32::from_rgb(255, 121, 198))
                                    .corner_radius(4.0),
                                )
                                .clicked()
                            {
                                self.cancel_task(&task.id);
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Host suivant")
                                            .size(10.0)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(Color32::from_rgb(241, 196, 15))
                                    .corner_radius(4.0),
                                )
                                .clicked()
                            {
                                self.skip_to_next_host(&task.id);
                            }
                        }
                        if finished {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Retirer")
                                            .size(10.0)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(Color32::from_rgb(68, 71, 90))
                                    .corner_radius(4.0),
                                )
                                .clicked()
                            {
                                self.forget_task(&task.id);
                            }
                        }
                    });
                });
            });
    }

    fn render_cf_banner(&self, ui: &mut egui::Ui) {
        if !self.cf_refreshing.load(Ordering::SeqCst) {
            return;
        }
        egui::Frame::NONE
            .fill(Color32::from_rgb(241, 196, 15))
            .corner_radius(8.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Cloudflare bloque")
                            .size(14.0)
                            .color(Color32::BLACK)
                            .strong(),
                    );
                    ui.label(
                        RichText::new("Va dans la fenêtre Chrome et clique sur \"Je suis humain\" si le challenge apparaît. Tu as 5 min — les téléchargements reprennent automatiquement dès le challenge résolu.")
                            .size(12.0)
                            .color(Color32::BLACK),
                    );
                });
            });
        ui.add_space(10.0);
    }

    fn render_anime_card(
        &mut self,
        ui: &mut egui::Ui,
        anime_idx: usize,
        ctx: &egui::Context,
    ) {
        let (image_url, expanded, downloaded) = {
            let anime = &self.animes[anime_idx];
            (
                anime
                    .anime
                    .affiche_small
                    .clone()
                    .unwrap_or_else(|| anime.anime.affiche.clone()),
                anime.expanded,
                anime.is_downloaded,
            )
        };

        let texture = self.load_image_from_db(&image_url, ctx);
        let host_options = self.host_options_for(&self.animes[anime_idx].anime.clone());

        let mut toggle_expanded = false;
        let mut dl_actions: Vec<(&'static str, Option<String>, Option<usize>)> = Vec::new();
        let mut ep_dl_actions: Vec<(&'static str, Option<String>, usize, usize)> = Vec::new();
        let mut open_url: Option<String> = None;
        let mut rating_changed: Option<Option<f32>> = None;
        let mut comment_committed = false;
        let mut status_changed: Option<Option<UserStatus>> = None;
        let mut tag_to_add: Option<String> = None;
        let mut tag_to_remove: Option<String> = None;
        let mut watched_toggled: Vec<(usize, usize, bool)> = Vec::new();
        let mut export_m3u = false;

        egui::Frame::NONE
            .fill(Color32::from_rgb(40, 42, 54))
            .corner_radius(12.0)
            .inner_margin(15.0)
            .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(60, 62, 74)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let img_size = Vec2::new(140.0, 200.0);
                    if let Some(texture) = &texture {
                        ui.image((texture.id(), img_size));
                    } else {
                        let (rect, _) = ui.allocate_exact_size(img_size, egui::Sense::hover());
                        ui.painter()
                            .rect_filled(rect, 8.0, Color32::from_rgb(30, 32, 44));
                    }

                    ui.add_space(20.0);

                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.heading(
                                RichText::new(&self.animes[anime_idx].anime.title)
                                    .size(22.0)
                                    .color(Color32::from_rgb(139, 233, 253))
                                    .strong(),
                            );
                            if downloaded {
                                ui.add_space(8.0);
                                egui::Frame::NONE
                                    .fill(Color32::from_rgb(80, 250, 123))
                                    .corner_radius(4.0)
                                    .inner_margin(egui::vec2(6.0, 2.0))
                                    .show(ui, |ui| {
                                        ui.label(
                                            RichText::new("DL")
                                                .size(10.0)
                                                .color(Color32::BLACK)
                                                .strong(),
                                        );
                                    });
                            }
                            let src_lbl = match self.animes[anime_idx].source {
                                AnimeSource::Franime => None,
                                AnimeSource::Uniquestream => Some(("US", Color32::from_rgb(139, 233, 253))),
                                AnimeSource::Voiranime => Some(("VA", Color32::from_rgb(255, 184, 108))),
                            };
                            if let Some((lbl, color)) = src_lbl {
                                ui.add_space(6.0);
                                egui::Frame::NONE
                                    .fill(color)
                                    .corner_radius(4.0)
                                    .inner_margin(egui::vec2(6.0, 2.0))
                                    .show(ui, |ui| {
                                        ui.label(
                                            RichText::new(lbl)
                                                .size(10.0)
                                                .color(Color32::BLACK)
                                                .strong(),
                                        );
                                    });
                            }
                        });

                        let anime_ref = &self.animes[anime_idx].anime;
                        if !anime_ref.title_o.is_empty() && anime_ref.title_o != anime_ref.title {
                            ui.label(
                                RichText::new(&anime_ref.title_o)
                                    .size(14.0)
                                    .color(Color32::from_rgb(150, 150, 160))
                                    .italics(),
                            );
                        }

                        ui.add_space(8.0);

                        ui.horizontal(|ui| {
                            if !anime_ref.note.is_empty() {
                                ui.label(
                                    RichText::new(format!("Note {}", anime_ref.note))
                                        .color(Color32::from_rgb(241, 250, 140))
                                        .strong(),
                                );
                                ui.separator();
                            }
                            ui.label(
                                RichText::new(&anime_ref.start_date)
                                    .color(Color32::from_rgb(150, 150, 160)),
                            );
                            ui.separator();
                            ui.label(
                                RichText::new(format!(
                                    "{} ép · {} saison(s)",
                                    self.animes[anime_idx].total_episodes(),
                                    anime_ref.saisons.len()
                                ))
                                .color(Color32::from_rgb(150, 150, 160)),
                            );
                        });

                        ui.add_space(6.0);

                        ui.horizontal(|ui| {
                            let display = &self.animes[anime_idx];
                            if display.has_vo {
                                badge(ui, "VO", Color32::from_rgb(80, 250, 123), Color32::BLACK);
                            }
                            if display.has_vf {
                                badge(ui, "VF", Color32::from_rgb(255, 121, 198), Color32::BLACK);
                            }
                            if display.anime.nsfw {
                                badge(ui, "NSFW", Color32::from_rgb(255, 85, 85), Color32::WHITE);
                            }
                        });

                        ui.add_space(8.0);

                        let themes: Vec<String> = self.animes[anime_idx]
                            .anime
                            .themes
                            .iter()
                            .take(5)
                            .cloned()
                            .collect();
                        if !themes.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                for theme in themes {
                                    egui::Frame::NONE
                                        .fill(Color32::from_rgb(68, 71, 90))
                                        .corner_radius(5.0)
                                        .inner_margin(egui::vec2(8.0, 4.0))
                                        .show(ui, |ui| {
                                            ui.label(
                                                RichText::new(theme)
                                                    .size(11.0)
                                                    .color(Color32::from_rgb(189, 147, 249)),
                                            );
                                        });
                                }
                            });
                            ui.add_space(8.0);
                        }

                        let description = truncate_str(
                            &self.animes[anime_idx].anime.description,
                            240,
                        );
                        ui.label(
                            RichText::new(description)
                                .size(13.0)
                                .color(Color32::from_rgb(200, 200, 210)),
                        );

                        ui.add_space(12.0);

                        ui.horizontal(|ui| {
                            let saisons: Vec<(String, usize)> = self.animes[anime_idx]
                                .anime
                                .saisons
                                .iter()
                                .enumerate()
                                .map(|(i, s)| (s.title.clone(), i))
                                .collect();
                            let has_vo = self.animes[anime_idx].has_vo;
                            let has_vf = self.animes[anime_idx].has_vf;

                            ui.menu_button(
                                RichText::new("Télécharger")
                                    .size(14.0)
                                    .color(Color32::WHITE)
                                    .strong(),
                                |ui| {
                                    ui.set_min_width(220.0);
                                    if has_vo {
                                        ui.menu_button("Tout l'anime — VO", |ui| {
                                            host_menu(ui, &host_options, |host| {
                                                dl_actions.push(("vo", host, None));
                                            });
                                        });
                                    }
                                    if has_vf {
                                        ui.menu_button("Tout l'anime — VF", |ui| {
                                            host_menu(ui, &host_options, |host| {
                                                dl_actions.push(("vf", host, None));
                                            });
                                        });
                                    }
                                    if saisons.len() > 1 {
                                        ui.separator();
                                        for (title, idx) in &saisons {
                                            let label_vo =
                                                format!("Saison « {} » — VO", title);
                                            let label_vf =
                                                format!("Saison « {} » — VF", title);
                                            if has_vo {
                                                let idx = *idx;
                                                ui.menu_button(label_vo, |ui| {
                                                    host_menu(ui, &host_options, |host| {
                                                        dl_actions
                                                            .push(("vo", host, Some(idx)));
                                                    });
                                                });
                                            }
                                            if has_vf {
                                                let idx = *idx;
                                                ui.menu_button(label_vf, |ui| {
                                                    host_menu(ui, &host_options, |host| {
                                                        dl_actions
                                                            .push(("vf", host, Some(idx)));
                                                    });
                                                });
                                            }
                                        }
                                    }
                                },
                            );

                            let expand_label = if expanded {
                                "Masquer les épisodes"
                            } else {
                                "Voir les épisodes"
                            };
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(expand_label)
                                            .size(13.0)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(Color32::from_rgb(68, 71, 90))
                                    .corner_radius(6.0),
                                )
                                .clicked()
                            {
                                toggle_expanded = true;
                            }

                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Source")
                                            .size(13.0)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(Color32::from_rgb(189, 147, 249))
                                    .corner_radius(6.0),
                                )
                                .clicked()
                            {
                                open_url = Some(
                                    self.animes[anime_idx]
                                        .anime
                                        .source_url
                                        .replace("/api/edge", ""),
                                );
                            }
                            if self.animes[anime_idx].is_downloaded
                                && ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new("Export m3u")
                                                .size(13.0)
                                                .color(Color32::WHITE),
                                        )
                                        .fill(Color32::from_rgb(80, 250, 123))
                                        .corner_radius(6.0),
                                    )
                                    .clicked()
                            {
                                export_m3u = true;
                            }
                        });

                        ui.add_space(10.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Ma note")
                                    .size(12.0)
                                    .color(Color32::from_rgb(150, 150, 160)),
                            );
                            let display = &mut self.animes[anime_idx];
                            let current = display.user_rating.unwrap_or(0.0);
                            for i in 1..=10 {
                                let filled = (i as f32) <= current + 0.001;
                                let glyph = if filled { "★" } else { "☆" };
                                let color = if filled {
                                    Color32::from_rgb(241, 196, 15)
                                } else {
                                    Color32::from_rgb(100, 100, 110)
                                };
                                let resp = ui.add(
                                    egui::Label::new(
                                        RichText::new(glyph).size(18.0).color(color),
                                    )
                                    .sense(egui::Sense::click()),
                                );
                                if resp.clicked() {
                                    let new_val = if (current - i as f32).abs() < 0.1 {
                                        None
                                    } else {
                                        Some(i as f32)
                                    };
                                    display.user_rating = new_val;
                                    rating_changed = Some(new_val);
                                }
                            }
                            if let Some(r) = display.user_rating {
                                ui.label(
                                    RichText::new(format!("{}/10", r as u32))
                                        .size(11.0)
                                        .color(Color32::from_rgb(150, 150, 160)),
                                );
                                if ui
                                    .small_button(RichText::new("Effacer").size(10.0))
                                    .clicked()
                                {
                                    display.user_rating = None;
                                    rating_changed = Some(None);
                                }
                            }
                        });

                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Statut")
                                    .size(12.0)
                                    .color(Color32::from_rgb(150, 150, 160)),
                            );
                            let display = &mut self.animes[anime_idx];
                            let current_label = display
                                .user_status
                                .map(|s| s.label())
                                .unwrap_or("—");
                            egui::ComboBox::from_id_salt(("status_combo", anime_idx))
                                .selected_text(current_label)
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_label(display.user_status.is_none(), "—")
                                        .clicked()
                                    {
                                        if display.user_status.is_some() {
                                            display.user_status = None;
                                            status_changed = Some(None);
                                        }
                                    }
                                    for s in UserStatus::all() {
                                        let sel = display.user_status == Some(s);
                                        if ui.selectable_label(sel, s.label()).clicked() {
                                            display.user_status = Some(s);
                                            status_changed = Some(Some(s));
                                        }
                                    }
                                });
                            if let Some(s) = display.user_status {
                                badge(ui, s.label(), s.color(), Color32::BLACK);
                            }
                        });

                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new("Tags")
                                    .size(12.0)
                                    .color(Color32::from_rgb(150, 150, 160)),
                            );
                            let display = &mut self.animes[anime_idx];
                            let existing: Vec<String> = display.user_tags.clone();
                            for tag in existing {
                                let resp = ui.add(
                                    egui::Button::new(
                                        RichText::new(format!("{} ×", tag))
                                            .size(10.0)
                                            .color(Color32::BLACK),
                                    )
                                    .fill(Color32::from_rgb(241, 196, 15))
                                    .corner_radius(4.0),
                                );
                                if resp.clicked() {
                                    display.user_tags.retain(|t| t != &tag);
                                    tag_to_remove = Some(tag);
                                }
                            }
                            let display = &mut self.animes[anime_idx];
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut display.tag_input)
                                    .hint_text("ajouter…")
                                    .desired_width(120.0),
                            );
                            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                let t = display.tag_input.trim().to_string();
                                if !t.is_empty() && !display.user_tags.contains(&t) {
                                    display.user_tags.push(t.clone());
                                    tag_to_add = Some(t);
                                }
                                display.tag_input.clear();
                            }
                        });

                        let display = &mut self.animes[anime_idx];
                        let response = ui.add(
                            egui::TextEdit::multiline(&mut display.user_comment)
                                .hint_text("Commentaire personnel (Cmd+Enter pour sauver)…")
                                .desired_rows(2)
                                .desired_width(f32::INFINITY),
                        );
                        if response.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        {
                            comment_committed = true;
                        }
                        if ui.button("Sauver le commentaire").clicked() {
                            comment_committed = true;
                        }
                    });
                });

                if expanded {
                    ui.add_space(15.0);
                    ui.separator();
                    ui.add_space(10.0);

                    type EpInfo = (String, Vec<String>, Vec<String>);
                    let saisons_data: Vec<(String, Vec<EpInfo>)> = self.animes[anime_idx]
                        .anime
                        .saisons
                        .iter()
                        .map(|s| {
                            (
                                s.title.clone(),
                                s.episodes
                                    .iter()
                                    .map(|e| {
                                        let vo_hosts: Vec<String> = e
                                            .lang
                                            .vo
                                            .lecteurs
                                            .iter()
                                            .filter(|n| {
                                                n.as_str() != "hd"
                                                    && n.as_str() != "TELECHARGEMENT UNIQUE"
                                            })
                                            .cloned()
                                            .collect();
                                        let vf_hosts: Vec<String> = e
                                            .lang
                                            .vf
                                            .lecteurs
                                            .iter()
                                            .filter(|n| {
                                                n.as_str() != "hd"
                                                    && n.as_str() != "TELECHARGEMENT UNIQUE"
                                            })
                                            .cloned()
                                            .collect();
                                        (e.title.clone(), vo_hosts, vf_hosts)
                                    })
                                    .collect(),
                            )
                        })
                        .collect();
                    let has_vo = self.animes[anime_idx].has_vo;
                    let has_vf = self.animes[anime_idx].has_vf;

                    if saisons_data.is_empty() {
                        ui.label(
                            RichText::new("Aucune saison disponible")
                                .size(13.0)
                                .color(Color32::from_rgb(150, 150, 160))
                                .italics(),
                        );
                    } else {
                        for (s_idx, (title, episodes)) in saisons_data.iter().enumerate() {
                            egui::Frame::NONE
                                .fill(Color32::from_rgb(50, 52, 64))
                                .corner_radius(8.0)
                                .inner_margin(12.0)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(title)
                                                .size(15.0)
                                                .color(Color32::from_rgb(139, 233, 253))
                                                .strong(),
                                        );
                                        ui.label(
                                            RichText::new(format!(
                                                "{} épisodes",
                                                episodes.len()
                                            ))
                                            .size(12.0)
                                            .color(Color32::from_rgb(150, 150, 160)),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if has_vf {
                                                    ui.menu_button("Saison VF", |ui| {
                                                        host_menu(ui, &host_options, |host| {
                                                            dl_actions
                                                                .push(("vf", host, Some(s_idx)));
                                                        });
                                                    });
                                                }
                                                if has_vo {
                                                    ui.menu_button("Saison VO", |ui| {
                                                        host_menu(ui, &host_options, |host| {
                                                            dl_actions
                                                                .push(("vo", host, Some(s_idx)));
                                                        });
                                                    });
                                                }
                                            },
                                        );
                                    });

                                    ui.add_space(6.0);

                                    for (e_idx, (ep_title, vo_hosts, vf_hosts)) in
                                        episodes.iter().enumerate()
                                    {
                                        let ep_has_vo = !vo_hosts.is_empty();
                                        let ep_has_vf = !vf_hosts.is_empty();
                                        let was_watched = self.animes[anime_idx]
                                            .watched_eps
                                            .contains(&(s_idx, e_idx));
                                        let mut is_watched = was_watched;
                                        ui.horizontal(|ui| {
                                            if ui.checkbox(&mut is_watched, "").changed() {
                                                if is_watched {
                                                    self.animes[anime_idx]
                                                        .watched_eps
                                                        .insert((s_idx, e_idx));
                                                } else {
                                                    self.animes[anime_idx]
                                                        .watched_eps
                                                        .remove(&(s_idx, e_idx));
                                                }
                                                watched_toggled.push((s_idx, e_idx, is_watched));
                                            }
                                            ui.label(
                                                RichText::new(format!("E{:02}", e_idx + 1))
                                                    .size(11.0)
                                                    .color(if is_watched {
                                                        Color32::from_rgb(100, 110, 130)
                                                    } else {
                                                        Color32::from_rgb(241, 250, 140)
                                                    })
                                                    .strong(),
                                            );
                                            ui.label(
                                                RichText::new(ep_title)
                                                    .size(12.0)
                                                    .color(if is_watched {
                                                        Color32::from_rgb(120, 130, 140)
                                                    } else {
                                                        Color32::from_rgb(200, 200, 210)
                                                    }),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(
                                                    egui::Align::Center,
                                                ),
                                                |ui| {
                                                    if !ep_has_vo && !ep_has_vf {
                                                        ui.label(
                                                            RichText::new("indisponible")
                                                                .size(10.0)
                                                                .color(Color32::from_rgb(
                                                                    150, 150, 160,
                                                                ))
                                                                .italics(),
                                                        );
                                                        return;
                                                    }
                                                    if ep_has_vf {
                                                        ui.menu_button(
                                                            RichText::new("VF")
                                                                .size(11.0)
                                                                .color(Color32::from_rgb(
                                                                    255, 121, 198,
                                                                ))
                                                                .strong(),
                                                            |ui| {
                                                                host_menu(
                                                                    ui,
                                                                    vf_hosts,
                                                                    |host| {
                                                                        ep_dl_actions.push((
                                                                            "vf",
                                                                            host,
                                                                            s_idx,
                                                                            e_idx,
                                                                        ));
                                                                    },
                                                                );
                                                            },
                                                        );
                                                    }
                                                    if ep_has_vo {
                                                        ui.menu_button(
                                                            RichText::new("VO")
                                                                .size(11.0)
                                                                .color(Color32::from_rgb(
                                                                    80, 250, 123,
                                                                ))
                                                                .strong(),
                                                            |ui| {
                                                                host_menu(
                                                                    ui,
                                                                    vo_hosts,
                                                                    |host| {
                                                                        ep_dl_actions.push((
                                                                            "vo",
                                                                            host,
                                                                            s_idx,
                                                                            e_idx,
                                                                        ));
                                                                    },
                                                                );
                                                            },
                                                        );
                                                    }
                                                },
                                            );
                                        });
                                    }
                                });
                            ui.add_space(6.0);
                        }
                    }
                }
            });

        if toggle_expanded {
            self.animes[anime_idx].expanded = !self.animes[anime_idx].expanded;
            if self.animes[anime_idx].expanded {
                self.trigger_us_load(anime_idx);
                self.trigger_va_load(anime_idx);
            }
        }
        if !dl_actions.is_empty() || !ep_dl_actions.is_empty() {
            self.trigger_us_load(anime_idx);
            self.trigger_va_load(anime_idx);
        }

        let anime_id_f64 = self.animes[anime_idx].anime.id;
        if let Some(new_rating) = rating_changed {
            let db = self.db.clone();
            self.runtime.spawn(async move {
                let guard = db.lock().await;
                let _ = guard.set_user_rating(anime_id_f64, new_rating);
            });
        }
        if comment_committed {
            let comment = self.animes[anime_idx].user_comment.clone();
            let value = if comment.trim().is_empty() {
                None
            } else {
                Some(comment)
            };
            let db = self.db.clone();
            self.runtime.spawn(async move {
                let guard = db.lock().await;
                let _ = guard.set_user_comment(anime_id_f64, value.as_deref());
            });
        }

        for (lang, host, season) in dl_actions {
            let anime_clone = self.animes[anime_idx].anime.clone();
            self.enqueue_anime(&anime_clone, lang, host.as_deref(), season);
        }
        for (lang, host, s_idx, e_idx) in ep_dl_actions {
            let anime_clone = self.animes[anime_idx].anime.clone();
            self.enqueue_download(&anime_clone, s_idx, e_idx, lang, host.as_deref());
        }

        if let Some(new_status) = status_changed {
            let key = new_status.map(|s| s.as_db_key());
            let db = self.db.clone();
            self.runtime.spawn(async move {
                let guard = db.lock().await;
                let _ = guard.set_user_status(anime_id_f64, key);
            });
        }

        if let Some(tag) = tag_to_add {
            let db = self.db.clone();
            self.runtime.spawn(async move {
                let guard = db.lock().await;
                let _ = guard.add_tag(anime_id_f64, &tag);
            });
        }
        if let Some(tag) = tag_to_remove {
            let db = self.db.clone();
            self.runtime.spawn(async move {
                let guard = db.lock().await;
                let _ = guard.remove_tag(anime_id_f64, &tag);
            });
        }

        for (s_idx, e_idx, watched) in watched_toggled {
            let db = self.db.clone();
            self.runtime.spawn(async move {
                let guard = db.lock().await;
                let _ = guard.set_episode_watched(anime_id_f64, s_idx, e_idx, watched);
            });
        }

        if let Some(url) = open_url {
            if let Err(e) = open::that(&url) {
                eprintln!("Erreur ouverture URL: {}", e);
            }
        }

        if export_m3u {
            let db = self.db.clone();
            let anime = self.animes[anime_idx].anime.clone();
            let dl_dir = self.settings.effective_download_dir();
            let downloads = self.runtime.block_on(async move {
                let guard = db.lock().await;
                guard.downloads_for_anime(anime.id).unwrap_or_default()
            });
            if downloads.is_empty() {
                applog::log_event(
                    applog::LogSource::App,
                    applog::LogLevel::Warn,
                    "Pas de fichiers téléchargés pour cet anime",
                );
            } else {
                let anime_name = if self.animes[anime_idx].anime.title_o.is_empty() {
                    self.animes[anime_idx].anime.title.clone()
                } else {
                    self.animes[anime_idx].anime.title_o.clone()
                };
                let mut content = String::from("#EXTM3U\n");
                for (s_idx, e_idx, lang, path) in &downloads {
                    content.push_str(&format!(
                        "#EXTINF:-1,{} S{:02}E{:02} ({})\n{}\n",
                        anime_name,
                        s_idx + 1,
                        e_idx + 1,
                        lang.to_uppercase(),
                        path
                    ));
                }
                let m3u_path = dl_dir.join(format!(
                    "{}.m3u",
                    sanitize_path_segment(&anime_name)
                ));
                match std::fs::write(&m3u_path, content) {
                    Ok(()) => {
                        applog::log_event(
                            applog::LogSource::App,
                            applog::LogLevel::Info,
                            format!("Playlist écrite: {}", m3u_path.display()),
                        );
                        let _ = open::that(&m3u_path);
                    }
                    Err(e) => {
                        applog::log_event(
                            applog::LogSource::App,
                            applog::LogLevel::Error,
                            format!("Écriture m3u échouée: {}", e),
                        );
                    }
                }
            }
        }
    }
}

fn badge(ui: &mut egui::Ui, text: &str, bg: Color32, fg: Color32) {
    egui::Frame::NONE
        .fill(bg)
        .corner_radius(4.0)
        .inner_margin(egui::vec2(8.0, 3.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(fg).strong());
        });
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

fn host_menu<F: FnMut(Option<String>)>(
    ui: &mut egui::Ui,
    hosts: &[String],
    mut on_pick: F,
) {
    ui.set_min_width(160.0);
    if ui.button("Auto (préférence)").clicked() {
        on_pick(None);
        ui.close_menu();
    }
    for host in hosts {
        if ui.button(host).clicked() {
            on_pick(Some(host.clone()));
            ui.close_menu();
        }
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_speed(bytes_per_sec: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    if bytes_per_sec >= MB {
        format!("{:.1} MB/s", bytes_per_sec as f64 / MB as f64)
    } else if bytes_per_sec >= KB {
        format!("{:.0} KB/s", bytes_per_sec as f64 / KB as f64)
    } else {
        format!("{} B/s", bytes_per_sec)
    }
}

fn format_eta(seconds: u64) -> String {
    if seconds == 0 {
        return "—".to_string();
    }
    if seconds < 60 {
        return format!("{}s", seconds);
    }
    let m = seconds / 60;
    let s = seconds % 60;
    if m < 60 {
        return format!("{}m{:02}", m, s);
    }
    let h = m / 60;
    let m = m % 60;
    format!("{}h{:02}", h, m)
}

async fn run_image_backfill(db: Arc<AsyncMutex<Database>>) -> SyncOutcome {
    let img_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36")
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let mut targets: Vec<(String, Option<String>)> = Vec::new();
    {
        let guard = db.lock().await;
        if let Ok(animes) = guard.load_animes() {
            for a in &animes {
                let url = a
                    .affiche_small
                    .clone()
                    .unwrap_or_else(|| a.affiche.clone());
                if !url.is_empty() {
                    targets.push((url, None));
                }
            }
        }
        if let Ok(us) = guard.load_us_animes() {
            for row in &us {
                if let Ok(series) =
                    serde_json::from_str::<uniquestream::BrowseSeries>(&row.json_data)
                {
                    if let Some(img) = series.image {
                        targets.push((
                            img,
                            Some("https://anime.uniquestream.net/".to_string()),
                        ));
                    }
                }
            }
        }
        if let Ok(va) = guard.load_va_animes() {
            for row in &va {
                if let Ok(series) = serde_json::from_str::<voiranime::VaSeries>(&row.json_data) {
                    if let Some(img) = series.image {
                        targets.push((img, Some("https://voir-anime.to/".to_string())));
                    }
                }
            }
        }
    }
    let total = targets.len();
    applog::log_event(
        applog::LogSource::App,
        applog::LogLevel::Info,
        format!("backfill images — {} urls candidates", total),
    );

    let mut total_saved = 0usize;
    for chunk in targets.chunks(50) {
        let items: Vec<(String, Option<String>)> = chunk.to_vec();
        let n = cache_images_batch(&db, &img_client, items).await;
        total_saved += n;
        applog::log_event(
            applog::LogSource::App,
            applog::LogLevel::Info,
            format!("backfill images — +{} (total {})", n, total_saved),
        );
    }
    applog::log_event(
        applog::LogSource::App,
        applog::LogLevel::Info,
        format!("backfill images fini — {} sauvées", total_saved),
    );
    SyncOutcome::Success {
        saved: total_saved,
        total,
    }
}

async fn cache_images_batch(
    db: &Arc<AsyncMutex<Database>>,
    client: &reqwest::Client,
    items: Vec<(String, Option<String>)>,
) -> usize {
    let mut to_fetch: Vec<(String, Option<String>)> = Vec::new();
    {
        let guard = db.lock().await;
        for (url, referer) in items {
            if url.is_empty() || !url.starts_with("http") {
                continue;
            }
            let present = guard.get_image(&url).ok().flatten().is_some();
            if !present {
                to_fetch.push((url, referer));
            }
        }
    }
    if to_fetch.is_empty() {
        return 0;
    }
    use futures::StreamExt;
    let fetches = futures::stream::iter(to_fetch.into_iter().map(|(url, referer)| {
        let client = client.clone();
        async move {
            let mut req = client.get(&url);
            if let Some(r) = referer.as_ref() {
                req = req.header("Referer", r);
            }
            let resp = match req.send().await {
                Ok(r) if r.status().is_success() => r,
                _ => return None,
            };
            let bytes = match resp.bytes().await {
                Ok(b) => b.to_vec(),
                Err(_) => return None,
            };
            let decoded = tokio::task::spawn_blocking(move || {
                image::load_from_memory(&bytes).map(|img| {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    (bytes, w, h)
                })
            })
            .await
            .ok()?;
            decoded.ok().map(|(b, w, h)| (url, b, w, h))
        }
    }))
    .buffer_unordered(6);
    let results: Vec<_> = fetches.collect().await;
    let mut saved = 0usize;
    let guard = db.lock().await;
    for opt in results {
        if let Some((url, bytes, w, h)) = opt {
            if guard.save_image(&url, &bytes, w, h).is_ok() {
                saved += 1;
            }
        }
    }
    saved
}

async fn run_us_sync(db: Arc<AsyncMutex<Database>>) -> SyncOutcome {
    let client = match uniquestream::UsClient::new() {
        Ok(c) => c,
        Err(e) => return SyncOutcome::Failure(format!("client uniquestream: {}", e)),
    };
    let total = match client.index_total().await {
        Ok(n) => n as usize,
        Err(e) => return SyncOutcome::Failure(format!("index uniquestream: {}", e)),
    };
    applog::log_event(
        applog::LogSource::App,
        applog::LogLevel::Info,
        format!("uniquestream sync début — index total = {}", total),
    );
    let mut saved = 0usize;
    let mut errors = 0usize;
    let mut offset: i64 = 0;
    let limit: i64 = 50;
    let img_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36")
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let mut images_cached = 0usize;
    loop {
        applog::log_event(
            applog::LogSource::App,
            applog::LogLevel::Info,
            format!("uniquestream browse offset={} limit={}", offset, limit),
        );
        let batch = match client.browse(offset, limit).await {
            Ok(b) => b,
            Err(e) => {
                applog::log_event(
                    applog::LogSource::App,
                    applog::LogLevel::Error,
                    format!("uniquestream browse offset={} FAIL: {}", offset, e),
                );
                break;
            }
        };
        applog::log_event(
            applog::LogSource::App,
            applog::LogLevel::Info,
            format!("uniquestream batch size = {}", batch.len()),
        );
        if batch.is_empty() {
            break;
        }
        let n = batch.len();
        {
            let guard = db.lock().await;
            for s in &batch {
                match guard.save_us_anime(s) {
                    Ok(()) => saved += 1,
                    Err(e) => {
                        errors += 1;
                        if errors <= 3 {
                            applog::log_event(
                                applog::LogSource::App,
                                applog::LogLevel::Error,
                                format!(
                                    "uniquestream save '{}' KO: {}",
                                    s.content_id, e
                                ),
                            );
                        }
                    }
                }
            }
        }
        let img_items: Vec<(String, Option<String>)> = batch
            .iter()
            .filter_map(|s| {
                s.image
                    .clone()
                    .map(|u| (u, Some("https://anime.uniquestream.net/".to_string())))
            })
            .collect();
        let n_imgs = cache_images_batch(&db, &img_client, img_items).await;
        images_cached += n_imgs;
        applog::log_event(
            applog::LogSource::App,
            applog::LogLevel::Info,
            format!("uniquestream images batch cached: {} (total {})", n_imgs, images_cached),
        );

        offset += n as i64;
        if (n as i64) < limit {
            break;
        }
    }
    applog::log_event(
        applog::LogSource::App,
        applog::LogLevel::Info,
        format!(
            "uniquestream list fini — {} sauvés, {} erreurs, {} images. Deep enrichissement…",
            saved, errors, images_cached
        ),
    );

    let pending = {
        let guard = db.lock().await;
        guard.load_us_animes_pending_deep().unwrap_or_default()
    };
    let total_pending = pending.len();
    applog::log_event(
        applog::LogSource::App,
        applog::LogLevel::Info,
        format!("uniquestream deep — {} animes à enrichir", total_pending),
    );

    use futures::StreamExt;
    let progress = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let progress_err = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tasks_stream = futures::stream::iter(pending.into_iter().map(|row| {
        let db = db.clone();
        let progress = progress.clone();
        let progress_err = progress_err.clone();
        async move {
            let client = match uniquestream::UsClient::new() {
                Ok(c) => c,
                Err(_) => {
                    progress_err.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };
            let content = match client.content(&row.content_id).await {
                Ok(c) => c,
                Err(uniquestream::UsError::NotASeries) => {
                    let guard = db.lock().await;
                    let _ = guard.mark_us_deep_done(&row.content_id);
                    drop(guard);
                    progress.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
                Err(e) => {
                    applog::log_event(
                        applog::LogSource::App,
                        applog::LogLevel::Warn,
                        format!(
                            "uniquestream detail '{}' KO: {}",
                            row.content_id, e
                        ),
                    );
                    progress_err.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };
            let (description, audio_locales, subtitle_locales, content_type) = match &content {
                uniquestream::ContentDetail::Series(s) => (
                    s.description.clone(),
                    s.audio_locales.clone(),
                    s.subtitle_locales.clone(),
                    "series",
                ),
                uniquestream::ContentDetail::Movie(m) => (
                    m.description.clone(),
                    m.audio_locales.clone(),
                    m.subtitle_locales.clone(),
                    "movie",
                ),
            };
            let detail_json = serde_json::json!({
                "content_id": row.content_id,
                "content_type": content_type,
                "description": description,
                "audio_locales": audio_locales,
                "subtitle_locales": subtitle_locales,
            });
            let mut original_v: serde_json::Value =
                serde_json::from_str(&row.json_data).unwrap_or(serde_json::json!({}));
            if let Some(desc) = description.clone() {
                if !desc.is_empty() {
                    original_v["info"] = serde_json::Value::String(desc);
                }
            }
            if let Some(al) = audio_locales.clone() {
                original_v["audio_locales"] = serde_json::json!(al);
                let has_dub = al.iter().any(|l| l != "ja-JP" && !l.is_empty());
                original_v["dubbed"] = serde_json::Value::Bool(has_dub);
                original_v["subbed"] = serde_json::Value::Bool(
                    al.iter().any(|l| l == "ja-JP") || al.is_empty(),
                );
            }
            if let Some(sl) = subtitle_locales.clone() {
                original_v["subtitle_locales"] = serde_json::json!(sl);
            }
            original_v["content_type"] = serde_json::Value::String(content_type.to_string());
            let updated_json = serde_json::to_string(&original_v).unwrap_or(row.json_data.clone());
            {
                let guard = db.lock().await;
                let _ = guard.update_us_json(&row.content_id, &updated_json);
                let _ = guard
                    .save_us_detail(&row.content_id, &detail_json.to_string());
                let _ = guard.mark_us_deep_done(&row.content_id);
            }
            let done = progress.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if done % 50 == 0 || done == total_pending {
                applog::log_event(
                    applog::LogSource::App,
                    applog::LogLevel::Info,
                    format!(
                        "uniquestream deep progress {}/{} (err: {})",
                        done,
                        total_pending,
                        progress_err.load(std::sync::atomic::Ordering::SeqCst)
                    ),
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
    }))
    .buffer_unordered(4);
    let _: Vec<()> = tasks_stream.collect().await;

    let deep_done = progress.load(std::sync::atomic::Ordering::SeqCst);
    let deep_err = progress_err.load(std::sync::atomic::Ordering::SeqCst);
    applog::log_event(
        applog::LogSource::App,
        applog::LogLevel::Info,
        format!(
            "uniquestream deep fini — {} enrichis, {} erreurs",
            deep_done, deep_err
        ),
    );

    SyncOutcome::Success {
        saved: saved + deep_done,
        total: total + total_pending,
    }
}

async fn run_sync(db: Arc<AsyncMutex<Database>>) -> SyncOutcome {
    let response = match reqwest::get("https://api.franime.fr/api/animes").await {
        Ok(r) => r,
        Err(e) => return SyncOutcome::Failure(format!("requête API: {}", e)),
    };

    let json_text = match response.text().await {
        Ok(t) => t,
        Err(e) => return SyncOutcome::Failure(format!("lecture réponse: {}", e)),
    };

    let animes: FullAnimeslist = match serde_json::from_str(&json_text) {
        Ok(a) => a,
        Err(e) => return SyncOutcome::Failure(format!("parsing JSON: {}", e)),
    };

    let total = animes.len();
    let mut saved = 0;

    for anime in animes {
        {
            let guard = db.lock().await;
            if guard.save_or_update_anime(&anime).is_ok() {
                saved += 1;
            }
        }

        let image_url = anime
            .affiche_small
            .as_ref()
            .unwrap_or(&anime.affiche)
            .clone();

        let needs_download = {
            let guard = db.lock().await;
            guard.get_image(&image_url).ok().flatten().is_none()
        };

        if needs_download {
            if let Ok(img_response) = reqwest::get(&image_url).await {
                if let Ok(img_bytes) = img_response.bytes().await {
                    let bytes_vec = img_bytes.to_vec();
                    let decoded = tokio::task::spawn_blocking(move || {
                        image::load_from_memory(&bytes_vec).map(|img| {
                            let rgba = img.to_rgba8();
                            let (w, h) = rgba.dimensions();
                            (bytes_vec, w, h)
                        })
                    })
                    .await;

                    if let Ok(Ok((bytes_vec, w, h))) = decoded {
                        let guard = db.lock().await;
                        let _ = guard.save_image(&image_url, &bytes_vec, w, h);
                    }
                }
            }
        }
    }

    SyncOutcome::Success { saved, total }
}

impl eframe::App for AnimeDownloaderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_sync_results();
        self.drain_us_load_results();
        self.drain_va_load_results();
        self.drain_image_loads(ctx);
        self.apply_theme(ctx);
        self.handle_shortcuts(ctx);
        let syncing = self.is_syncing.load(Ordering::SeqCst);

        let active_downloads = self.active_downloads_count();
        if ctx.input(|i| i.viewport().close_requested())
            && !self.confirmed_close
            && active_downloads > 0
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.show_close_confirm = true;
        }

        self.render_settings_window(ctx);
        self.render_close_confirm_window(ctx);

        egui::SidePanel::left("nav_panel")
            .resizable(true)
            .default_width(280.0)
            .min_width(240.0)
            .max_width(360.0)
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(30, 32, 44))
                    .inner_margin(16.0),
            )
            .show(ctx, |ui| {
                self.render_nav_panel(ui, ctx, syncing);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(40, 42, 54))
                    .inner_margin(20.0),
            )
            .show(ctx, |ui| {
                self.render_cf_banner(ui);

                let active_count = active_downloads;
                ui.horizontal(|ui| {
                    let mode_tab = |ui: &mut egui::Ui,
                                    label: String,
                                    selected: bool|
                     -> egui::Response {
                        ui.add(
                            egui::Button::new(
                                RichText::new(label)
                                    .size(14.0)
                                    .color(if selected {
                                        Color32::BLACK
                                    } else {
                                        Color32::from_rgb(200, 200, 210)
                                    })
                                    .strong(),
                            )
                            .fill(if selected {
                                Color32::from_rgb(189, 147, 249)
                            } else {
                                Color32::from_rgb(50, 52, 64)
                            })
                            .corner_radius(6.0)
                            .min_size(Vec2::new(140.0, 32.0)),
                        )
                    };
                    if mode_tab(
                        ui,
                        "Catalogue".to_string(),
                        self.view_mode == ViewMode::Catalogue,
                    )
                    .clicked()
                    {
                        self.view_mode = ViewMode::Catalogue;
                    }
                    if mode_tab(
                        ui,
                        "Ma liste".to_string(),
                        self.view_mode == ViewMode::MaListe,
                    )
                    .clicked()
                    {
                        self.view_mode = ViewMode::MaListe;
                    }
                    let dl_label = if active_count > 0 {
                        format!("Téléchargements ({})", active_count)
                    } else {
                        "Téléchargements".to_string()
                    };
                    if mode_tab(
                        ui,
                        dl_label,
                        self.view_mode == ViewMode::Telechargements,
                    )
                    .clicked()
                    {
                        self.view_mode = ViewMode::Telechargements;
                    }
                    let logs_count = applog::instance().len();
                    let logs_label = if logs_count > 0 {
                        format!("Logs ({})", logs_count)
                    } else {
                        "Logs".to_string()
                    };
                    if mode_tab(ui, logs_label, self.view_mode == ViewMode::Logs).clicked() {
                        self.view_mode = ViewMode::Logs;
                    }
                    if mode_tab(
                        ui,
                        "Stats".to_string(),
                        self.view_mode == ViewMode::Stats,
                    )
                    .clicked()
                    {
                        self.view_mode = ViewMode::Stats;
                    }
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| match self.view_mode {
                            ViewMode::Telechargements => {
                                let total = self.total_download_speed();
                                if total > 0 {
                                    ui.label(
                                        RichText::new(format!(
                                            "Vitesse totale : {}",
                                            format_speed(total)
                                        ))
                                        .size(13.0)
                                        .color(Color32::from_rgb(139, 233, 253))
                                        .strong(),
                                    );
                                }
                            }
                            _ => {
                                ui.label(
                                    RichText::new(format!(
                                        "{} affiché(s) · {} en base",
                                        self.current_view_indices().len(),
                                        self.animes.len()
                                    ))
                                    .size(12.0)
                                    .color(Color32::from_rgb(150, 150, 160)),
                                );
                            }
                        },
                    );
                });

                ui.add_space(15.0);

                match self.view_mode {
                    ViewMode::Telechargements => self.render_downloads_page(ui),
                    ViewMode::Logs => self.render_logs_page(ui),
                    ViewMode::Stats => self.render_stats_page(ui),
                    _ => {
                        let mut sort_changed = false;
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Trier par")
                                    .size(11.0)
                                    .color(Color32::from_rgb(150, 150, 160)),
                            );
                            egui::ComboBox::from_id_salt("catalogue_sort")
                                .selected_text(self.sort_mode.label())
                                .show_ui(ui, |ui| {
                                    for m in SortMode::all() {
                                        if ui
                                            .selectable_label(self.sort_mode == m, m.label())
                                            .clicked()
                                        {
                                            self.sort_mode = m;
                                            sort_changed = true;
                                        }
                                    }
                                });
                            if ui
                                .button(if self.sort_descending {
                                    RichText::new("Décroissant").size(11.0)
                                } else {
                                    RichText::new("Croissant").size(11.0)
                                })
                                .clicked()
                            {
                                self.sort_descending = !self.sort_descending;
                                sort_changed = true;
                            }
                        });
                        if sort_changed {
                            self.filter_animes();
                        }
                        ui.add_space(10.0);

                        let indices = self.current_view_indices();
                        if indices.is_empty() {
                            egui::ScrollArea::vertical()
                                .auto_shrink([false; 2])
                                .show(ui, |ui| {
                                    ui.vertical_centered(|ui| {
                                        ui.add_space(80.0);
                                        let msg = if self.view_mode == ViewMode::MaListe {
                                            "Ta liste est vide. Note ou commente un anime, ou télécharge un épisode pour qu'il apparaisse ici."
                                        } else if self.animes.is_empty() {
                                            "Aucun anime en base. Clique sur \"Synchroniser API\" dans le panneau de gauche."
                                        } else {
                                            "Aucun anime ne correspond à ta recherche."
                                        };
                                        ui.label(
                                            RichText::new(msg)
                                                .size(14.0)
                                                .color(Color32::from_rgb(150, 150, 160)),
                                        );
                                    });
                                });
                        } else {
                            let expanded_indices: Vec<usize> = indices
                                .iter()
                                .copied()
                                .filter(|&i| self.animes[i].expanded)
                                .collect();
                            if !expanded_indices.is_empty() {
                                egui::ScrollArea::vertical()
                                    .auto_shrink([false; 2])
                                    .show(ui, |ui| {
                                        for idx in expanded_indices {
                                            self.render_anime_card(ui, idx, ctx);
                                            ui.add_space(12.0);
                                        }
                                    });
                            } else {
                                let row_h = 380.0_f32;
                                let total = indices.len();
                                egui::ScrollArea::vertical()
                                    .auto_shrink([false; 2])
                                    .show_rows(ui, row_h, total, |ui, range| {
                                        for visible_i in range {
                                            let idx = indices[visible_i];
                                            self.render_anime_card(ui, idx, ctx);
                                        }
                                    });
                            }
                        }
                    }
                }
            });

        if syncing
            || self.view_mode == ViewMode::Telechargements
            || self.view_mode == ViewMode::Logs
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
    }
}

impl AnimeDownloaderApp {
    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let is_text_focused = ctx.memory(|m| m.focused().is_some());
        let (slash, num1, num2, num3, num4, num5, esc, cmd_comma, cmd_l) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Slash) && !is_text_focused,
                i.modifiers.command && i.key_pressed(egui::Key::Num1),
                i.modifiers.command && i.key_pressed(egui::Key::Num2),
                i.modifiers.command && i.key_pressed(egui::Key::Num3),
                i.modifiers.command && i.key_pressed(egui::Key::Num4),
                i.modifiers.command && i.key_pressed(egui::Key::Num5),
                i.key_pressed(egui::Key::Escape),
                i.modifiers.command && i.key_pressed(egui::Key::Comma),
                i.modifiers.command && i.key_pressed(egui::Key::L),
            )
        });
        if slash {
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("nav_search_field")));
        }
        if num1 {
            self.view_mode = ViewMode::Catalogue;
        }
        if num2 {
            self.view_mode = ViewMode::MaListe;
        }
        if num3 {
            self.view_mode = ViewMode::Telechargements;
        }
        if num4 {
            self.view_mode = ViewMode::Logs;
        }
        if num5 {
            self.view_mode = ViewMode::Stats;
        }
        if cmd_comma {
            self.settings_pending = self.settings.clone();
            self.show_settings = true;
        }
        if cmd_l {
            self.view_mode = ViewMode::Logs;
        }
        if esc {
            self.show_settings = false;
            self.show_close_confirm = false;
        }
    }

    fn apply_theme(&self, ctx: &egui::Context) {
        let want_dark = self.settings.theme_dark;
        let is_dark = ctx.style().visuals.dark_mode;
        if want_dark != is_dark {
            ctx.set_visuals(if want_dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
        }
    }

    fn active_downloads_count(&self) -> usize {
        self.task_view
            .lock()
            .unwrap()
            .iter()
            .filter(|t| {
                matches!(
                    t.status,
                    DlStatus::Queued | DlStatus::Extracting | DlStatus::Downloading(_)
                )
            })
            .count()
    }

    fn total_download_speed(&self) -> u64 {
        self.task_view
            .lock()
            .unwrap()
            .iter()
            .filter_map(|t| match &t.status {
                DlStatus::Downloading(p) => Some(p.speed_bytes_per_sec),
                _ => None,
            })
            .sum()
    }

    fn render_downloads_page(&mut self, ui: &mut egui::Ui) {
        let tasks: Vec<DownloadTask> = self.task_view.lock().unwrap().clone();

        let active = tasks
            .iter()
            .filter(|t| {
                matches!(
                    t.status,
                    DlStatus::Queued | DlStatus::Extracting | DlStatus::Downloading(_)
                )
            })
            .count();
        let done = tasks
            .iter()
            .filter(|t| matches!(t.status, DlStatus::Completed))
            .count();
        let failed = tasks
            .iter()
            .filter(|t| matches!(t.status, DlStatus::Failed(_)))
            .count();
        let cancelled = tasks
            .iter()
            .filter(|t| matches!(t.status, DlStatus::Cancelled))
            .count();
        let total_speed: u64 = tasks
            .iter()
            .filter_map(|t| match &t.status {
                DlStatus::Downloading(p) => Some(p.speed_bytes_per_sec),
                _ => None,
            })
            .sum();
        let total_downloaded: u64 = tasks
            .iter()
            .filter_map(|t| match &t.status {
                DlStatus::Downloading(p) => Some(p.downloaded),
                _ => None,
            })
            .sum();
        let total_remaining: u64 = tasks
            .iter()
            .filter_map(|t| match &t.status {
                DlStatus::Downloading(p) if p.total > p.downloaded => {
                    Some(p.total - p.downloaded)
                }
                _ => None,
            })
            .sum();
        let aggregate_eta = if total_speed > 0 {
            total_remaining / total_speed
        } else {
            0
        };

        egui::Frame::NONE
            .fill(Color32::from_rgb(35, 37, 48))
            .corner_radius(10.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    stat_card(
                        ui,
                        "Actifs",
                        &active.to_string(),
                        Color32::from_rgb(139, 233, 253),
                    );
                    stat_card(
                        ui,
                        "Terminés",
                        &done.to_string(),
                        Color32::from_rgb(80, 250, 123),
                    );
                    stat_card(
                        ui,
                        "Échec",
                        &failed.to_string(),
                        Color32::from_rgb(255, 85, 85),
                    );
                    stat_card(
                        ui,
                        "Annulés",
                        &cancelled.to_string(),
                        Color32::from_rgb(180, 120, 120),
                    );
                });

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Vitesse totale")
                            .size(11.0)
                            .color(Color32::from_rgb(150, 150, 160)),
                    );
                    ui.label(
                        RichText::new(format_speed(total_speed))
                            .size(20.0)
                            .color(Color32::from_rgb(139, 233, 253))
                            .strong(),
                    );
                    ui.separator();
                    ui.label(
                        RichText::new("Téléchargé")
                            .size(11.0)
                            .color(Color32::from_rgb(150, 150, 160)),
                    );
                    ui.label(
                        RichText::new(format_bytes(total_downloaded))
                            .size(14.0)
                            .color(Color32::from_rgb(200, 200, 210))
                            .strong(),
                    );
                    if aggregate_eta > 0 {
                        ui.separator();
                        ui.label(
                            RichText::new("ETA agrégé")
                                .size(11.0)
                                .color(Color32::from_rgb(150, 150, 160)),
                        );
                        ui.label(
                            RichText::new(format_eta(aggregate_eta))
                                .size(14.0)
                                .color(Color32::from_rgb(241, 250, 140))
                                .strong(),
                        );
                    }
                });
            });

        ui.add_space(12.0);

        ui.horizontal(|ui| {
            let filter_btn = |ui: &mut egui::Ui,
                              label: &str,
                              filter: DownloadsFilter,
                              current: DownloadsFilter|
             -> egui::Response {
                let selected = filter == current;
                ui.add(
                    egui::Button::new(
                        RichText::new(label)
                            .size(12.0)
                            .color(if selected {
                                Color32::BLACK
                            } else {
                                Color32::WHITE
                            }),
                    )
                    .fill(if selected {
                        Color32::from_rgb(139, 233, 253)
                    } else {
                        Color32::from_rgb(50, 52, 64)
                    })
                    .corner_radius(5.0)
                    .min_size(Vec2::new(80.0, 26.0)),
                )
            };

            if filter_btn(
                ui,
                &format!("Tous ({})", tasks.len()),
                DownloadsFilter::All,
                self.downloads_filter,
            )
            .clicked()
            {
                self.downloads_filter = DownloadsFilter::All;
            }
            if filter_btn(
                ui,
                &format!("Actifs ({})", active),
                DownloadsFilter::Active,
                self.downloads_filter,
            )
            .clicked()
            {
                self.downloads_filter = DownloadsFilter::Active;
            }
            if filter_btn(
                ui,
                &format!("Échec ({})", failed),
                DownloadsFilter::Failed,
                self.downloads_filter,
            )
            .clicked()
            {
                self.downloads_filter = DownloadsFilter::Failed;
            }
            if filter_btn(
                ui,
                &format!("Terminés ({})", done),
                DownloadsFilter::Done,
                self.downloads_filter,
            )
            .clicked()
            {
                self.downloads_filter = DownloadsFilter::Done;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("Nettoyer terminés")
                                .size(11.0)
                                .color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(68, 71, 90))
                        .corner_radius(5.0),
                    )
                    .clicked()
                {
                    self.clear_finished();
                }
            });
        });

        ui.add_space(10.0);

        let filter = self.downloads_filter;
        let filtered: Vec<&DownloadTask> = tasks
            .iter()
            .filter(|t| match filter {
                DownloadsFilter::All => true,
                DownloadsFilter::Active => matches!(
                    t.status,
                    DlStatus::Queued | DlStatus::Extracting | DlStatus::Downloading(_)
                ),
                DownloadsFilter::Failed => matches!(t.status, DlStatus::Failed(_)),
                DownloadsFilter::Done => matches!(t.status, DlStatus::Completed),
            })
            .collect();

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                if filtered.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(60.0);
                        ui.label(
                            RichText::new("Aucun téléchargement dans ce filtre")
                                .size(13.0)
                                .color(Color32::from_rgb(150, 150, 160))
                                .italics(),
                        );
                    });
                } else {
                    for task in filtered {
                        self.render_task_row(ui, task);
                        ui.add_space(6.0);
                    }
                }
            });
    }

    fn render_close_confirm_window(&mut self, ctx: &egui::Context) {
        if !self.show_close_confirm {
            return;
        }
        let active = self.active_downloads_count();
        let mut want_cancel = false;
        let mut want_quit = false;
        let mut open = true;

        egui::Window::new(RichText::new("Téléchargements en cours").size(15.0).strong())
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(Color32::from_rgb(40, 42, 54))
                    .inner_margin(20.0),
            )
            .show(ctx, |ui| {
                ui.set_min_width(380.0);
                ui.label(
                    RichText::new(format!(
                        "{} téléchargement(s) sont encore actifs.",
                        active
                    ))
                    .size(13.0)
                    .color(Color32::from_rgb(220, 220, 230)),
                );
                ui.label(
                    RichText::new(
                        "Si tu quittes maintenant, les fichiers partiels resteront sur le disque et les transferts seront perdus.",
                    )
                    .size(12.0)
                    .color(Color32::from_rgb(180, 180, 190)),
                );

                ui.add_space(14.0);

                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("Continuer les téléchargements")
                                    .size(12.0)
                                    .color(Color32::WHITE)
                                    .strong(),
                            )
                            .fill(Color32::from_rgb(80, 250, 123))
                            .corner_radius(6.0)
                            .min_size(Vec2::new(0.0, 32.0)),
                        )
                        .clicked()
                    {
                        want_cancel = true;
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("Quitter quand même")
                                    .size(12.0)
                                    .color(Color32::WHITE),
                            )
                            .fill(Color32::from_rgb(255, 85, 85))
                            .corner_radius(6.0)
                            .min_size(Vec2::new(0.0, 32.0)),
                        )
                        .clicked()
                    {
                        want_quit = true;
                    }
                });
            });

        if want_cancel || !open {
            self.show_close_confirm = false;
        }
        if want_quit {
            self.confirmed_close = true;
            self.show_close_confirm = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn render_stats_page(&mut self, ui: &mut egui::Ui) {
        let total = self.animes.len();
        let rated: Vec<f32> = self
            .animes
            .iter()
            .filter_map(|a| a.user_rating)
            .collect();
        let commented = self.animes.iter().filter(|a| !a.user_comment.is_empty()).count();
        let downloaded_animes = self.animes.iter().filter(|a| a.is_downloaded).count();
        let with_status = self.animes.iter().filter(|a| a.user_status.is_some()).count();
        let tagged = self.animes.iter().filter(|a| !a.user_tags.is_empty()).count();

        let rating_avg = if !rated.is_empty() {
            rated.iter().sum::<f32>() / rated.len() as f32
        } else {
            0.0
        };
        let rating_median = if !rated.is_empty() {
            let mut sorted = rated.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            sorted[sorted.len() / 2]
        } else {
            0.0
        };

        let db = self.db.clone();
        let history = self.runtime.block_on(async move {
            let guard = db.lock().await;
            guard.download_history().unwrap_or_default()
        });
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let week_ago = now - 7 * 86400;
        let recent_dl = history.iter().filter(|(_, _, _, _, t)| *t >= week_ago).count();
        let total_eps_dl = history.len();
        let vo_count = history.iter().filter(|(_, _, _, l, _)| l == "vo").count();
        let vf_count = history.iter().filter(|(_, _, _, l, _)| l == "vf").count();

        let mut status_counts: std::collections::HashMap<UserStatus, usize> =
            std::collections::HashMap::new();
        for a in &self.animes {
            if let Some(s) = a.user_status {
                *status_counts.entry(s).or_insert(0) += 1;
            }
        }

        let mut rating_dist = [0usize; 10];
        for r in &rated {
            let idx = ((*r as usize).max(1).min(10)) - 1;
            rating_dist[idx] += 1;
        }

        let mut theme_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for a in &self.animes {
            let engaged = a.user_rating.map(|r| r >= 7.0).unwrap_or(false)
                || a.is_downloaded
                || a.user_status == Some(UserStatus::Termine);
            if engaged {
                for t in &a.anime.themes {
                    if !t.is_empty() {
                        *theme_counts.entry(t.clone()).or_insert(0) += 1;
                    }
                }
            }
        }
        let mut top_themes: Vec<(String, usize)> = theme_counts.into_iter().collect();
        top_themes.sort_by(|a, b| b.1.cmp(&a.1));
        top_themes.truncate(15);

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    stat_card(ui, "Animes", &total.to_string(), Color32::from_rgb(139, 233, 253));
                    stat_card(ui, "Notés", &rated.len().to_string(), Color32::from_rgb(241, 196, 15));
                    stat_card(ui, "Commentés", &commented.to_string(), Color32::from_rgb(189, 147, 249));
                    stat_card(ui, "Avec statut", &with_status.to_string(), Color32::from_rgb(80, 250, 123));
                    stat_card(ui, "Tagués", &tagged.to_string(), Color32::from_rgb(241, 196, 15));
                    stat_card(ui, "Téléchargés", &downloaded_animes.to_string(), Color32::from_rgb(80, 250, 123));
                    stat_card(ui, "Épisodes DL", &total_eps_dl.to_string(), Color32::from_rgb(255, 121, 198));
                    stat_card(ui, "DL 7j", &recent_dl.to_string(), Color32::from_rgb(139, 233, 253));
                });

                ui.add_space(20.0);
                section_header(ui, "Tes notes");
                if rated.is_empty() {
                    ui.label(
                        RichText::new("Aucune note user. Note un anime depuis sa carte.")
                            .size(12.0)
                            .color(Color32::from_rgb(150, 150, 160))
                            .italics(),
                    );
                } else {
                    ui.horizontal(|ui| {
                        stat_card(ui, "Moyenne", &format!("{:.1}", rating_avg), Color32::from_rgb(241, 196, 15));
                        stat_card(ui, "Médiane", &format!("{:.1}", rating_median), Color32::from_rgb(241, 196, 15));
                    });
                    ui.add_space(8.0);
                    let max_count = *rating_dist.iter().max().unwrap_or(&1).max(&1);
                    for (i, count) in rating_dist.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{:>2}/10", i + 1))
                                    .size(11.0)
                                    .monospace()
                                    .color(Color32::from_rgb(150, 150, 160)),
                            );
                            let frac = *count as f32 / max_count as f32;
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(220.0, 12.0),
                                egui::Sense::hover(),
                            );
                            ui.painter().rect_filled(
                                rect,
                                3.0,
                                Color32::from_rgb(50, 52, 64),
                            );
                            let mut filled = rect;
                            filled.set_width(rect.width() * frac);
                            ui.painter()
                                .rect_filled(filled, 3.0, Color32::from_rgb(241, 196, 15));
                            ui.label(
                                RichText::new(count.to_string())
                                    .size(11.0)
                                    .color(Color32::from_rgb(200, 200, 210)),
                            );
                        });
                    }
                }

                ui.add_space(20.0);
                section_header(ui, "Statut");
                if status_counts.is_empty() {
                    ui.label(
                        RichText::new("Aucun statut défini.")
                            .size(12.0)
                            .color(Color32::from_rgb(150, 150, 160))
                            .italics(),
                    );
                } else {
                    ui.horizontal_wrapped(|ui| {
                        for s in UserStatus::all() {
                            let c = *status_counts.get(&s).unwrap_or(&0);
                            stat_card(ui, s.label(), &c.to_string(), s.color());
                        }
                    });
                }

                ui.add_space(20.0);
                section_header(ui, "Langue téléchargée");
                ui.horizontal(|ui| {
                    stat_card(ui, "VO", &vo_count.to_string(), Color32::from_rgb(80, 250, 123));
                    stat_card(ui, "VF", &vf_count.to_string(), Color32::from_rgb(255, 121, 198));
                });

                ui.add_space(20.0);
                section_header(ui, "Top thèmes (animes engagés)");
                if top_themes.is_empty() {
                    ui.label(
                        RichText::new("Note un anime ≥ 7, télécharge ou marque comme Terminé pour alimenter ces stats.")
                            .size(12.0)
                            .color(Color32::from_rgb(150, 150, 160))
                            .italics(),
                    );
                } else {
                    let max_theme = top_themes.first().map(|(_, c)| *c).unwrap_or(1).max(1);
                    for (theme, count) in &top_themes {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(theme)
                                    .size(11.0)
                                    .color(Color32::from_rgb(189, 147, 249))
                                    .strong(),
                            );
                            let frac = *count as f32 / max_theme as f32;
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(180.0, 10.0),
                                egui::Sense::hover(),
                            );
                            ui.painter().rect_filled(
                                rect,
                                3.0,
                                Color32::from_rgb(50, 52, 64),
                            );
                            let mut filled = rect;
                            filled.set_width(rect.width() * frac);
                            ui.painter()
                                .rect_filled(filled, 3.0, Color32::from_rgb(189, 147, 249));
                            ui.label(
                                RichText::new(count.to_string())
                                    .size(11.0)
                                    .color(Color32::from_rgb(200, 200, 210)),
                            );
                        });
                    }
                }
            });
    }

    fn render_logs_page(&mut self, ui: &mut egui::Ui) {
        let log = applog::instance();
        let entries = log.snapshot();

        egui::Frame::NONE
            .fill(Color32::from_rgb(35, 37, 48))
            .corner_radius(8.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new("Source")
                            .size(11.0)
                            .color(Color32::from_rgb(150, 150, 160)),
                    );
                    self.log_source_chip(ui, None, "Toutes");
                    self.log_source_chip(ui, Some(applog::LogSource::App), "App");
                    self.log_source_chip(ui, Some(applog::LogSource::Sidecar), "Sidecar");
                    self.log_source_chip(ui, Some(applog::LogSource::Python), "Python");

                    ui.add_space(16.0);

                    ui.label(
                        RichText::new("Niveau")
                            .size(11.0)
                            .color(Color32::from_rgb(150, 150, 160)),
                    );
                    self.log_level_chip(ui, None, "Tous");
                    self.log_level_chip(ui, Some(applog::LogLevel::Info), "Info");
                    self.log_level_chip(ui, Some(applog::LogLevel::Warn), "Warn");
                    self.log_level_chip(ui, Some(applog::LogLevel::Error), "Erreur");
                });

                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Filtrer")
                            .size(11.0)
                            .color(Color32::from_rgb(150, 150, 160)),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.logs_filter_query)
                            .hint_text("texte à chercher dans le message…")
                            .desired_width(360.0),
                    );
                    ui.checkbox(&mut self.logs_autoscroll, "Auto-scroll");

                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Vider")
                                            .size(11.0)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(Color32::from_rgb(255, 85, 85))
                                    .corner_radius(5.0),
                                )
                                .clicked()
                            {
                                log.clear();
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Copier")
                                            .size(11.0)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(Color32::from_rgb(68, 71, 90))
                                    .corner_radius(5.0),
                                )
                                .clicked()
                            {
                                let text = entries
                                    .iter()
                                    .map(format_log_line)
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                ui.ctx().copy_text(text);
                            }
                        },
                    );
                });
            });

        ui.add_space(10.0);

        let query = self.logs_filter_query.to_lowercase();
        let src_filter = self.logs_filter_source;
        let lvl_filter = self.logs_filter_level;
        let filtered: Vec<&applog::LogEntry> = entries
            .iter()
            .filter(|e| {
                src_filter.map_or(true, |s| e.source == s)
                    && lvl_filter.map_or(true, |l| e.level == l)
                    && (query.is_empty() || e.message.to_lowercase().contains(&query))
            })
            .collect();

        ui.label(
            RichText::new(format!(
                "{} / {} entrée(s)",
                filtered.len(),
                entries.len()
            ))
            .size(11.0)
            .color(Color32::from_rgb(150, 150, 160)),
        );

        ui.add_space(6.0);

        let scroll = egui::ScrollArea::vertical().auto_shrink([false; 2]);
        let autoscroll = self.logs_autoscroll;
        let scroll = if autoscroll {
            scroll.stick_to_bottom(true)
        } else {
            scroll
        };
        scroll.show(ui, |ui| {
            if filtered.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.label(
                        RichText::new("Aucune entrée. Lance un téléchargement pour générer des logs.")
                            .size(13.0)
                            .color(Color32::from_rgb(150, 150, 160))
                            .italics(),
                    );
                });
                return;
            }
            for entry in &filtered {
                render_log_row(ui, entry);
            }
        });
    }

    fn log_source_chip(
        &mut self,
        ui: &mut egui::Ui,
        source: Option<applog::LogSource>,
        label: &str,
    ) {
        let selected = self.logs_filter_source == source;
        if ui
            .add(
                egui::Button::new(
                    RichText::new(label).size(11.0).color(if selected {
                        Color32::BLACK
                    } else {
                        Color32::WHITE
                    }),
                )
                .fill(if selected {
                    Color32::from_rgb(189, 147, 249)
                } else {
                    Color32::from_rgb(50, 52, 64)
                })
                .corner_radius(4.0)
                .min_size(Vec2::new(60.0, 22.0)),
            )
            .clicked()
        {
            self.logs_filter_source = source;
        }
    }

    fn log_level_chip(
        &mut self,
        ui: &mut egui::Ui,
        level: Option<applog::LogLevel>,
        label: &str,
    ) {
        let selected = self.logs_filter_level == level;
        let color = level
            .map(|l| match l {
                applog::LogLevel::Info => Color32::from_rgb(139, 233, 253),
                applog::LogLevel::Warn => Color32::from_rgb(241, 196, 15),
                applog::LogLevel::Error => Color32::from_rgb(255, 85, 85),
            })
            .unwrap_or(Color32::from_rgb(200, 200, 210));
        if ui
            .add(
                egui::Button::new(
                    RichText::new(label).size(11.0).color(if selected {
                        Color32::BLACK
                    } else {
                        Color32::WHITE
                    }),
                )
                .fill(if selected {
                    color
                } else {
                    Color32::from_rgb(50, 52, 64)
                })
                .corner_radius(4.0)
                .min_size(Vec2::new(50.0, 22.0)),
            )
            .clicked()
        {
            self.logs_filter_level = level;
        }
    }

    fn current_view_indices(&self) -> Vec<usize> {
        self.filtered_indices
            .iter()
            .filter(|&&i| {
                if self.view_mode == ViewMode::MaListe {
                    let a = &self.animes[i];
                    a.user_rating.is_some()
                        || !a.user_comment.is_empty()
                        || a.is_downloaded
                        || a.user_status.is_some()
                        || !a.user_tags.is_empty()
                        || !a.watched_eps.is_empty()
                } else {
                    true
                }
            })
            .copied()
            .collect()
    }

    fn render_nav_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, syncing: bool) {
        ui.heading(
            RichText::new("franime_dl")
                .size(22.0)
                .color(Color32::from_rgb(189, 147, 249))
                .strong(),
        );
        ui.label(
            RichText::new("Bibliothèque locale")
                .size(11.0)
                .color(Color32::from_rgb(120, 120, 130)),
        );

        ui.add_space(16.0);

        ui.label(
            RichText::new("Recherche")
                .size(11.0)
                .color(Color32::from_rgb(150, 150, 160)),
        );
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.search_query)
                .id(egui::Id::new("nav_search_field"))
                .hint_text("Titre… ( / pour focus)")
                .desired_width(f32::INFINITY),
        );
        if response.changed() {
            self.filter_animes();
        }

        ui.add_space(12.0);

        ui.label(
            RichText::new("Langue")
                .size(11.0)
                .color(Color32::from_rgb(150, 150, 160)),
        );
        let filters = [
            (LangFilter::All, "Toutes", Color32::from_rgb(100, 100, 110)),
            (LangFilter::VO, "VO uniquement", Color32::from_rgb(80, 250, 123)),
            (LangFilter::VF, "VF uniquement", Color32::from_rgb(255, 121, 198)),
            (LangFilter::Both, "VO + VF", Color32::from_rgb(139, 233, 253)),
        ];
        for (filter, label, color) in filters {
            let is_selected = self.lang_filter == filter;
            let btn = egui::Button::new(
                RichText::new(label)
                    .size(12.0)
                    .color(if is_selected {
                        Color32::BLACK
                    } else {
                        Color32::WHITE
                    }),
            )
            .fill(if is_selected {
                color
            } else {
                Color32::from_rgb(50, 52, 64)
            })
            .corner_radius(5.0)
            .min_size(Vec2::new(0.0, 26.0));
            if ui.add_sized([ui.available_width(), 26.0], btn).clicked() {
                self.lang_filter = filter;
                self.filter_animes();
            }
        }

        ui.add_space(12.0);

        let mut filters_changed = false;
        egui::CollapsingHeader::new(
            RichText::new("Filtres avancés")
                .size(12.0)
                .color(Color32::from_rgb(200, 200, 210))
                .strong(),
        )
        .default_open(false)
        .show(ui, |ui| {
            ui.label(
                RichText::new("Note user minimum")
                    .size(11.0)
                    .color(Color32::from_rgb(150, 150, 160)),
            );
            if ui
                .add(
                    egui::Slider::new(&mut self.min_user_rating, 0.0..=10.0)
                        .step_by(0.5)
                        .show_value(true)
                        .text("≥"),
                )
                .changed()
            {
                filters_changed = true;
            }

            ui.add_space(6.0);
            if ui
                .checkbox(&mut self.only_downloaded, "Seulement les téléchargés")
                .changed()
            {
                filters_changed = true;
            }
            if ui
                .checkbox(&mut self.hide_nsfw, "Cacher NSFW")
                .changed()
            {
                filters_changed = true;
            }

            ui.add_space(8.0);

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Thèmes")
                        .size(11.0)
                        .color(Color32::from_rgb(150, 150, 160)),
                );
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let any_selected = self.theme_filter_mode == ThemeFilterMode::Any;
                        if ui
                            .selectable_label(any_selected, "Au moins 1")
                            .clicked()
                        {
                            self.theme_filter_mode = ThemeFilterMode::Any;
                            filters_changed = true;
                        }
                        if ui
                            .selectable_label(!any_selected, "Tous")
                            .clicked()
                        {
                            self.theme_filter_mode = ThemeFilterMode::All;
                            filters_changed = true;
                        }
                    },
                );
            });

            if !self.selected_themes.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    let to_remove: Vec<String> = self.selected_themes.iter().cloned().collect();
                    for theme in &to_remove {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(format!("{} ×", theme))
                                        .size(10.0)
                                        .color(Color32::BLACK),
                                )
                                .fill(Color32::from_rgb(189, 147, 249))
                                .corner_radius(4.0),
                            )
                            .clicked()
                        {
                            self.selected_themes.remove(theme);
                            filters_changed = true;
                        }
                    }
                });
                if ui
                    .button(RichText::new("Effacer thèmes").size(10.0))
                    .clicked()
                {
                    self.selected_themes.clear();
                    filters_changed = true;
                }
                ui.add_space(4.0);
            }

            egui::ScrollArea::vertical()
                .max_height(180.0)
                .auto_shrink([false, true])
                .id_salt("themes_picker_scroll")
                .show(ui, |ui| {
                    let themes: Vec<String> = self
                        .all_themes_cache
                        .iter()
                        .filter(|t| !self.selected_themes.contains(*t))
                        .cloned()
                        .collect();
                    if themes.is_empty() && self.selected_themes.is_empty() {
                        ui.label(
                            RichText::new("Synchronise pour charger les thèmes")
                                .size(10.0)
                                .color(Color32::from_rgb(120, 120, 130))
                                .italics(),
                        );
                    } else {
                        ui.horizontal_wrapped(|ui| {
                            for theme in themes {
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(&theme)
                                                .size(10.0)
                                                .color(Color32::from_rgb(189, 147, 249)),
                                        )
                                        .fill(Color32::from_rgb(50, 52, 64))
                                        .corner_radius(4.0),
                                    )
                                    .clicked()
                                {
                                    self.selected_themes.insert(theme);
                                    filters_changed = true;
                                }
                            }
                        });
                    }
                });

            ui.add_space(8.0);
            ui.label(
                RichText::new("Statut user")
                    .size(11.0)
                    .color(Color32::from_rgb(150, 150, 160)),
            );
            ui.horizontal_wrapped(|ui| {
                for s in UserStatus::all() {
                    let selected = self.selected_statuses.contains(&s);
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(s.label()).size(10.0).color(if selected {
                                    Color32::BLACK
                                } else {
                                    Color32::WHITE
                                }),
                            )
                            .fill(if selected {
                                s.color()
                            } else {
                                Color32::from_rgb(50, 52, 64)
                            })
                            .corner_radius(4.0),
                        )
                        .clicked()
                    {
                        if selected {
                            self.selected_statuses.remove(&s);
                        } else {
                            self.selected_statuses.insert(s);
                        }
                        filters_changed = true;
                    }
                }
            });

            if !self.all_user_tags_cache.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Tags user (AND)")
                        .size(11.0)
                        .color(Color32::from_rgb(150, 150, 160)),
                );
                ui.horizontal_wrapped(|ui| {
                    let tags = self.all_user_tags_cache.clone();
                    for tag in tags {
                        let selected = self.selected_user_tags.contains(&tag);
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(&tag).size(10.0).color(if selected {
                                        Color32::BLACK
                                    } else {
                                        Color32::from_rgb(241, 196, 15)
                                    }),
                                )
                                .fill(if selected {
                                    Color32::from_rgb(241, 196, 15)
                                } else {
                                    Color32::from_rgb(50, 52, 64)
                                })
                                .corner_radius(4.0),
                            )
                            .clicked()
                        {
                            if selected {
                                self.selected_user_tags.remove(&tag);
                            } else {
                                self.selected_user_tags.insert(tag);
                            }
                            filters_changed = true;
                        }
                    }
                });
            }

            ui.add_space(6.0);
            if ui
                .button(RichText::new("Réinitialiser tous").size(11.0))
                .clicked()
            {
                self.min_user_rating = 0.0;
                self.only_downloaded = false;
                self.hide_nsfw = false;
                self.selected_themes.clear();
                self.theme_filter_mode = ThemeFilterMode::Any;
                self.selected_statuses.clear();
                self.selected_user_tags.clear();
                filters_changed = true;
            }
        });

        if filters_changed {
            self.filter_animes();
        }

        ui.add_space(12.0);

        let sync_label = if syncing { "Synchronisation…" } else { "Synchroniser API" };
        let sync_btn = egui::Button::new(
            RichText::new(sync_label)
                .size(13.0)
                .color(Color32::WHITE)
                .strong(),
        )
        .fill(if syncing {
            Color32::from_rgb(100, 100, 110)
        } else {
            Color32::from_rgb(80, 250, 123)
        })
        .corner_radius(6.0);
        if ui
            .add_enabled_ui(!syncing, |ui| {
                ui.add_sized([ui.available_width(), 32.0], sync_btn)
            })
            .inner
            .clicked()
        {
            self.sync_from_api(ctx.clone());
        }

        ui.add_space(6.0);
        let us_label = if syncing {
            "Sync en cours…"
        } else {
            "Sync uniquestream"
        };
        let us_btn = egui::Button::new(
            RichText::new(us_label)
                .size(13.0)
                .color(Color32::WHITE),
        )
        .fill(if syncing {
            Color32::from_rgb(100, 100, 110)
        } else {
            Color32::from_rgb(139, 233, 253)
        })
        .corner_radius(6.0);
        if ui
            .add_enabled_ui(!syncing, |ui| {
                ui.add_sized([ui.available_width(), 28.0], us_btn)
            })
            .inner
            .clicked()
        {
            self.sync_uniquestream(ctx.clone());
        }

        ui.add_space(6.0);
        let va_label = if syncing {
            "Sync en cours…"
        } else {
            "Sync voiranime"
        };
        let va_btn = egui::Button::new(
            RichText::new(va_label)
                .size(13.0)
                .color(Color32::BLACK),
        )
        .fill(if syncing {
            Color32::from_rgb(100, 100, 110)
        } else {
            Color32::from_rgb(255, 184, 108)
        })
        .corner_radius(6.0);
        if ui
            .add_enabled_ui(!syncing, |ui| {
                ui.add_sized([ui.available_width(), 28.0], va_btn)
            })
            .inner
            .clicked()
        {
            self.sync_voiranime(ctx.clone());
        }

        ui.add_space(6.0);
        let backfill_label = if syncing {
            "Sync en cours…"
        } else {
            "Backfill images"
        };
        let backfill_btn = egui::Button::new(
            RichText::new(backfill_label)
                .size(13.0)
                .color(Color32::WHITE),
        )
        .fill(if syncing {
            Color32::from_rgb(100, 100, 110)
        } else {
            Color32::from_rgb(98, 114, 164)
        })
        .corner_radius(6.0);
        if ui
            .add_enabled_ui(!syncing, |ui| {
                ui.add_sized([ui.available_width(), 28.0], backfill_btn)
            })
            .inner
            .clicked()
        {
            self.backfill_images(ctx.clone());
        }

        ui.add_space(6.0);
        let reload_btn = egui::Button::new(
            RichText::new("Recharger la base")
                .size(13.0)
                .color(Color32::WHITE),
        )
        .fill(Color32::from_rgb(68, 71, 90))
        .corner_radius(6.0);
        if ui
            .add_sized([ui.available_width(), 28.0], reload_btn)
            .clicked()
        {
            self.reload_from_db();
        }

        if !self.sync_status.is_empty() {
            ui.add_space(6.0);
            ui.label(
                RichText::new(&self.sync_status)
                    .size(11.0)
                    .color(Color32::from_rgb(241, 250, 140)),
            );
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            let settings_btn = egui::Button::new(
                RichText::new("Réglages")
                    .size(13.0)
                    .color(Color32::WHITE),
            )
            .fill(Color32::from_rgb(68, 71, 90))
            .corner_radius(6.0);
            if ui
                .add_sized([ui.available_width(), 32.0], settings_btn)
                .clicked()
            {
                self.settings_pending = self.settings.clone();
                self.show_settings = true;
            }
        });
    }

    fn render_settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let mut should_save = false;
        let mut should_cancel = false;
        let mut export_backup = false;
        let mut scan_orphans = false;
        let mut restart_sidecar = false;

        let dirty = self.settings_pending.max_concurrent_downloads
            != self.settings.max_concurrent_downloads
            || self.settings_pending.max_concurrent_extractions
                != self.settings.max_concurrent_extractions
            || self.settings_pending.preferred_lecteur_host != self.settings.preferred_lecteur_host
            || self.settings_pending.download_dir != self.settings.download_dir
            || self.settings_pending.chrome_headless != self.settings.chrome_headless
            || self.settings_pending.naming_format != self.settings.naming_format
            || self.settings_pending.skip_existing != self.settings.skip_existing
            || self.settings_pending.theme_dark != self.settings.theme_dark
            || self.settings_pending.notifications_enabled != self.settings.notifications_enabled
            || self.settings_pending.sidecar_warmup != self.settings.sidecar_warmup
            || self.settings_pending.consumet_base_url != self.settings.consumet_base_url
            || self.settings_pending.consumet_provider != self.settings.consumet_provider
            || self.settings_pending.consumet_enabled != self.settings.consumet_enabled
            || self.settings_pending.consumet_auto_fallback
                != self.settings.consumet_auto_fallback
            || self.settings_pending.anikuro_enabled != self.settings.anikuro_enabled
            || self.settings_pending.anikuro_provider != self.settings.anikuro_provider
            || self.settings_pending.anikuro_auto_fallback
                != self.settings.anikuro_auto_fallback
            || self.settings_pending.anikuro_prefer_dub != self.settings.anikuro_prefer_dub;

        egui::Window::new(RichText::new("Réglages").size(16.0).strong())
            .open(&mut open)
            .resizable(true)
            .default_width(520.0)
            .default_height(440.0)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(Color32::from_rgb(40, 42, 54))
                    .inner_margin(20.0),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .max_height(ui.available_height() - 60.0)
                    .show(ui, |ui| {
                        section_header(ui, "Téléchargements");
                        ui.label(
                            RichText::new("Nombre maximum de .mp4 téléchargés en parallèle.")
                                .size(11.0)
                                .color(Color32::from_rgb(150, 150, 160)),
                        );
                        ui.add(
                            egui::Slider::new(
                                &mut self.settings_pending.max_concurrent_downloads,
                                1..=16,
                            )
                            .clamping(egui::SliderClamping::Always)
                            .text("simultanés"),
                        );

                        ui.add_space(14.0);
                        section_header(ui, "Extraction");
                        ui.label(
                            RichText::new(
                                "Nombre de requêtes d'extraction iframe traitées en parallèle par le sidecar nodriver.",
                            )
                            .size(11.0)
                            .color(Color32::from_rgb(150, 150, 160)),
                        );
                        ui.add(
                            egui::Slider::new(
                                &mut self.settings_pending.max_concurrent_extractions,
                                1..=4,
                            )
                            .clamping(egui::SliderClamping::Always)
                            .text("simultanées"),
                        );

                        ui.add_space(14.0);
                        section_header(ui, "Lecteur préféré");
                        ui.label(
                            RichText::new(
                                "Premier host essayé pour chaque épisode. Les autres servent de fallback si le préféré échoue.",
                            )
                            .size(11.0)
                            .color(Color32::from_rgb(150, 150, 160)),
                        );
                        let hosts = ["", "sibnet", "sendvid", "filemoon"];
                        let labels = ["Auto (ordre du site)", "sibnet", "sendvid", "filemoon"];
                        egui::ComboBox::from_id_salt("preferred_host_combo_modal")
                            .selected_text(
                                labels[hosts
                                    .iter()
                                    .position(|h| {
                                        *h == self.settings_pending.preferred_lecteur_host
                                    })
                                    .unwrap_or(0)],
                            )
                            .width(280.0)
                            .show_ui(ui, |ui| {
                                for (i, host) in hosts.iter().enumerate() {
                                    ui.selectable_value(
                                        &mut self.settings_pending.preferred_lecteur_host,
                                        host.to_string(),
                                        labels[i],
                                    );
                                }
                            });

                        ui.add_space(14.0);
                        section_header(ui, "Stockage");
                        ui.label(
                            RichText::new(
                                "Dossier racine où les sous-dossiers download_VO/ et download_VF/ seront créés. Laisser vide pour utiliser le dossier de lancement.",
                            )
                            .size(11.0)
                            .color(Color32::from_rgb(150, 150, 160)),
                        );
                        ui.add(
                            egui::TextEdit::singleline(
                                &mut self.settings_pending.download_dir,
                            )
                            .hint_text("/chemin/absolu (vide = courant)")
                            .desired_width(f32::INFINITY),
                        );
                        ui.label(
                            RichText::new(format!(
                                "Effectif : {}",
                                self.settings_pending.effective_download_dir().display()
                            ))
                            .size(10.0)
                            .color(Color32::from_rgb(120, 120, 130))
                            .italics(),
                        );

                        ui.add_space(14.0);
                        section_header(ui, "Format des fichiers");
                        ui.label(
                            RichText::new(
                                "Plex : 'Anime - S01E12 - title [VO].mp4' (compat lecteurs). Simple : 'title.mp4'.",
                            )
                            .size(11.0)
                            .color(Color32::from_rgb(150, 150, 160)),
                        );
                        let formats = [("plex", "Plex"), ("simple", "Simple")];
                        egui::ComboBox::from_id_salt("naming_format_combo")
                            .selected_text(
                                formats
                                    .iter()
                                    .find(|(k, _)| *k == self.settings_pending.naming_format)
                                    .map(|(_, l)| *l)
                                    .unwrap_or("Plex"),
                            )
                            .width(200.0)
                            .show_ui(ui, |ui| {
                                for (k, l) in &formats {
                                    ui.selectable_value(
                                        &mut self.settings_pending.naming_format,
                                        k.to_string(),
                                        *l,
                                    );
                                }
                            });
                        ui.checkbox(
                            &mut self.settings_pending.skip_existing,
                            "Skip si le fichier existe déjà sur le disque",
                        );

                        ui.add_space(14.0);
                        section_header(ui, "Sidecar Chrome");
                        ui.label(
                            RichText::new(
                                "Mode headless : Chrome tourne en arrière-plan sans fenêtre visible. Utilise --headless=new (moderne). Désactive si tu dois résoudre un captcha à la main.",
                            )
                            .size(11.0)
                            .color(Color32::from_rgb(150, 150, 160)),
                        );
                        ui.checkbox(
                            &mut self.settings_pending.chrome_headless,
                            "Lancer Chrome en headless (new)",
                        );
                        ui.checkbox(
                            &mut self.settings_pending.sidecar_warmup,
                            "Préchauffer le sidecar au démarrage de l'app",
                        );

                        ui.add_space(8.0);
                        let metrics = self.runtime.block_on(async {
                            self.manager.sidecar().metrics().await
                        });
                        let uptime = if metrics.is_alive && metrics.started_at > 0 {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs() as i64)
                                .unwrap_or(0);
                            let s = (now - metrics.started_at).max(0) as u64;
                            format_eta(s)
                        } else {
                            "—".to_string()
                        };
                        ui.label(
                            RichText::new(format!(
                                "État : {}",
                                if metrics.is_alive {
                                    "actif"
                                } else {
                                    "arrêté"
                                }
                            ))
                            .size(11.0)
                            .color(if metrics.is_alive {
                                Color32::from_rgb(80, 250, 123)
                            } else {
                                Color32::from_rgb(150, 150, 160)
                            }),
                        );
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("Uptime: {}", uptime))
                                    .size(11.0)
                                    .color(Color32::from_rgb(180, 180, 190)),
                            );
                            ui.separator();
                            ui.label(
                                RichText::new(format!("CF solves: {}", metrics.cf_solves))
                                    .size(11.0)
                                    .color(Color32::from_rgb(180, 180, 190)),
                            );
                            ui.separator();
                            ui.label(
                                RichText::new(format!(
                                    "Fetch OK/KO: {}/{}",
                                    metrics.fetch_ok, metrics.fetch_err
                                ))
                                .size(11.0)
                                .color(Color32::from_rgb(180, 180, 190)),
                            );
                            ui.separator();
                            ui.label(
                                RichText::new(format!(
                                    "Refresh CF: {}",
                                    metrics.refresh_calls
                                ))
                                .size(11.0)
                                .color(Color32::from_rgb(180, 180, 190)),
                            );
                        });
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Redémarrer le sidecar")
                                        .size(12.0)
                                        .color(Color32::WHITE),
                                )
                                .fill(Color32::from_rgb(255, 121, 198))
                                .corner_radius(5.0),
                            )
                            .clicked()
                        {
                            restart_sidecar = true;
                        }

                        ui.add_space(14.0);
                        section_header(ui, "Notifications");
                        ui.checkbox(
                            &mut self.settings_pending.notifications_enabled,
                            "Notifications système à la fin d'un téléchargement",
                        );

                        ui.add_space(14.0);
                        section_header(ui, "Source alternative (Anikuro)");
                        ui.label(
                            RichText::new(
                                "Anikuro.to expose une API publique gratuite qui agrège animepahe / allanime / etc. Pas besoin de Docker ou de service à lancer. Utilisé en fallback si tous les hosts franime échouent.",
                            )
                            .size(11.0)
                            .color(Color32::from_rgb(150, 150, 160)),
                        );
                        ui.checkbox(
                            &mut self.settings_pending.anikuro_enabled,
                            "Activer Anikuro",
                        );
                        ui.checkbox(
                            &mut self.settings_pending.anikuro_auto_fallback,
                            "Fallback auto si tous les hosts franime échouent",
                        );
                        ui.checkbox(
                            &mut self.settings_pending.anikuro_prefer_dub,
                            "Préférer la VF (dub) si dispo, sinon VOSTFR",
                        );
                        ui.label(
                            RichText::new("Provider préféré")
                                .size(11.0)
                                .color(Color32::from_rgb(150, 150, 160)),
                        );
                        let ani_providers = ["animepahe", "allanime"];
                        egui::ComboBox::from_id_salt("anikuro_provider_combo")
                            .selected_text(&self.settings_pending.anikuro_provider)
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                for p in ani_providers {
                                    ui.selectable_value(
                                        &mut self.settings_pending.anikuro_provider,
                                        p.to_string(),
                                        p,
                                    );
                                }
                            });

                        ui.add_space(14.0);
                        section_header(ui, "Source alternative legacy (Consumet, DMCA'd)");
                        ui.label(
                            RichText::new(
                                "Cherche dans gogoanime/zoro/animepahe/9anime/etc. via une API Consumet quand les hosts franime ne marchent pas. Faut soit utiliser une instance publique (rapide à essayer mais peut être down), soit en héberger une localement (git clone consumet/api.consumet.org + npm i + npm start, sans Docker).",
                            )
                            .size(11.0)
                            .color(Color32::from_rgb(150, 150, 160)),
                        );
                        ui.checkbox(
                            &mut self.settings_pending.consumet_enabled,
                            "Activer Consumet",
                        );
                        ui.checkbox(
                            &mut self.settings_pending.consumet_auto_fallback,
                            "Fallback auto si tous les hosts franime échouent",
                        );
                        ui.label(
                            RichText::new("URL de base")
                                .size(11.0)
                                .color(Color32::from_rgb(150, 150, 160)),
                        );
                        ui.add(
                            egui::TextEdit::singleline(
                                &mut self.settings_pending.consumet_base_url,
                            )
                            .hint_text("ex. http://localhost:3000 ou https://api.consumet.org")
                            .desired_width(f32::INFINITY),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Utiliser l'instance publique")
                                            .size(10.0)
                                            .color(Color32::BLACK),
                                    )
                                    .fill(Color32::from_rgb(139, 233, 253))
                                    .corner_radius(4.0),
                                )
                                .clicked()
                            {
                                self.settings_pending.consumet_base_url =
                                    "https://api.consumet.org".to_string();
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Localhost:3000")
                                            .size(10.0)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(Color32::from_rgb(68, 71, 90))
                                    .corner_radius(4.0),
                                )
                                .clicked()
                            {
                                self.settings_pending.consumet_base_url =
                                    "http://localhost:3000".to_string();
                            }
                        });
                        ui.label(
                            RichText::new("Provider préféré")
                                .size(11.0)
                                .color(Color32::from_rgb(150, 150, 160)),
                        );
                        let providers = [
                            "gogoanime", "zoro", "animepahe", "9anime", "animekai",
                            "animefox", "marin",
                        ];
                        egui::ComboBox::from_id_salt("consumet_provider_combo")
                            .selected_text(&self.settings_pending.consumet_provider)
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                for p in providers {
                                    ui.selectable_value(
                                        &mut self.settings_pending.consumet_provider,
                                        p.to_string(),
                                        p,
                                    );
                                }
                            });

                        ui.add_space(14.0);
                        section_header(ui, "Sauvegarde et entretien");
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Exporter la base (JSON)")
                                        .size(12.0)
                                        .color(Color32::WHITE),
                                )
                                .fill(Color32::from_rgb(80, 250, 123))
                                .corner_radius(5.0),
                            )
                            .clicked()
                        {
                            export_backup = true;
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Scanner les orphelins")
                                        .size(12.0)
                                        .color(Color32::WHITE),
                                )
                                .fill(Color32::from_rgb(189, 147, 249))
                                .corner_radius(5.0),
                            )
                            .clicked()
                        {
                            scan_orphans = true;
                        }
                        ui.label(
                            RichText::new(
                                "Le scan vérifie les fichiers du dossier de téléchargement contre la base et logge les divergences.",
                            )
                            .size(10.0)
                            .color(Color32::from_rgb(120, 120, 130))
                            .italics(),
                        );

                        ui.add_space(14.0);
                        section_header(ui, "Thème");
                        ui.horizontal(|ui| {
                            if ui
                                .selectable_label(
                                    self.settings_pending.theme_dark,
                                    "Sombre",
                                )
                                .clicked()
                            {
                                self.settings_pending.theme_dark = true;
                            }
                            if ui
                                .selectable_label(
                                    !self.settings_pending.theme_dark,
                                    "Clair",
                                )
                                .clicked()
                            {
                                self.settings_pending.theme_dark = false;
                            }
                        });

                        ui.add_space(14.0);
                        ui.separator();
                        ui.label(
                            RichText::new(
                                "Les changements de parallélisme et du mode headless prennent effet au prochain démarrage du sidecar (donc à la prochaine résolution CF).",
                            )
                            .size(10.0)
                            .color(Color32::from_rgb(180, 150, 100))
                            .italics(),
                        );
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    let save_btn = egui::Button::new(
                        RichText::new("Sauver")
                            .size(13.0)
                            .color(Color32::WHITE)
                            .strong(),
                    )
                    .fill(if dirty {
                        Color32::from_rgb(80, 250, 123)
                    } else {
                        Color32::from_rgb(100, 100, 110)
                    })
                    .corner_radius(6.0)
                    .min_size(Vec2::new(100.0, 32.0));
                    if ui.add_enabled(dirty, save_btn).clicked() {
                        should_save = true;
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("Annuler").size(13.0).color(Color32::WHITE),
                            )
                            .fill(Color32::from_rgb(68, 71, 90))
                            .corner_radius(6.0)
                            .min_size(Vec2::new(100.0, 32.0)),
                        )
                        .clicked()
                    {
                        should_cancel = true;
                    }
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if dirty {
                                ui.label(
                                    RichText::new("Modifications non sauvées")
                                        .size(10.0)
                                        .color(Color32::from_rgb(241, 196, 15))
                                        .italics(),
                                );
                            }
                        },
                    );
                });
            });

        if should_save {
            let pending = self.settings_pending.clone();
            let db = self.db.clone();
            let pending_save = pending.clone();
            self.runtime.spawn(async move {
                let guard = db.lock().await;
                pending_save.save(&guard);
            });
            self.settings = pending;
        }
        if should_cancel {
            self.settings_pending = self.settings.clone();
            self.show_settings = false;
        }
        if !open {
            self.settings_pending = self.settings.clone();
            self.show_settings = false;
        }

        if export_backup {
            self.do_export_backup();
        }
        if scan_orphans {
            self.do_scan_orphans();
        }
        if restart_sidecar {
            let sidecar = self.manager.sidecar();
            self.runtime.spawn(async move {
                applog::log_event(
                    applog::LogSource::Sidecar,
                    applog::LogLevel::Warn,
                    "redémarrage du sidecar demandé",
                );
                sidecar.restart().await;
            });
        }
    }

    fn do_export_backup(&self) {
        let db = self.db.clone();
        let dir = self.settings.effective_download_dir();
        self.runtime.spawn(async move {
            let guard = db.lock().await;
            let notes = guard.load_user_notes().unwrap_or_default();
            let tags = guard.load_tags().unwrap_or_default();
            let watched = guard.load_watched().unwrap_or_default();
            let downloads = guard.download_history().unwrap_or_default();
            drop(guard);

            let mut payload = serde_json::Map::new();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            payload.insert("version".to_string(), serde_json::json!(1));
            payload.insert("exported_at".to_string(), serde_json::json!(now));

            let notes_json: serde_json::Map<String, serde_json::Value> = notes
                .into_iter()
                .map(|(k, n)| {
                    let id = f64::from_bits(k);
                    (
                        id.to_string(),
                        serde_json::json!({
                            "rating": n.rating,
                            "comment": n.comment,
                            "status": n.status,
                            "finished_at": n.finished_at,
                        }),
                    )
                })
                .collect();
            payload.insert("notes".to_string(), serde_json::Value::Object(notes_json));

            let tags_json: serde_json::Map<String, serde_json::Value> = tags
                .into_iter()
                .map(|(k, ts)| {
                    let id = f64::from_bits(k);
                    (id.to_string(), serde_json::json!(ts))
                })
                .collect();
            payload.insert("tags".to_string(), serde_json::Value::Object(tags_json));

            let watched_json: serde_json::Map<String, serde_json::Value> = watched
                .into_iter()
                .map(|(k, s)| {
                    let id = f64::from_bits(k);
                    let arr: Vec<[usize; 2]> = s.into_iter().map(|(a, b)| [a, b]).collect();
                    (id.to_string(), serde_json::json!(arr))
                })
                .collect();
            payload.insert("watched".to_string(), serde_json::Value::Object(watched_json));

            let dl_json: Vec<serde_json::Value> = downloads
                .into_iter()
                .map(|(id, s, e, l, t)| {
                    serde_json::json!({
                        "anime_id": id, "season_idx": s, "ep_idx": e,
                        "lang": l, "completed_at": t,
                    })
                })
                .collect();
            payload.insert("downloads".to_string(), serde_json::Value::Array(dl_json));

            let path = dir.join(format!("franime_backup_{}.json", now));
            match serde_json::to_string_pretty(&serde_json::Value::Object(payload))
                .map(|s| std::fs::write(&path, s))
            {
                Ok(Ok(())) => {
                    applog::log_event(
                        applog::LogSource::App,
                        applog::LogLevel::Info,
                        format!("Backup écrit: {}", path.display()),
                    );
                    let _ = open::that(&path);
                }
                Ok(Err(e)) => applog::log_event(
                    applog::LogSource::App,
                    applog::LogLevel::Error,
                    format!("Backup IO err: {}", e),
                ),
                Err(e) => applog::log_event(
                    applog::LogSource::App,
                    applog::LogLevel::Error,
                    format!("Backup JSON err: {}", e),
                ),
            }
        });
    }

    fn do_scan_orphans(&self) {
        let db = self.db.clone();
        let dl_root = self.settings.effective_download_dir();
        self.runtime.spawn(async move {
            let guard = db.lock().await;
            let entries = guard.all_downloads().unwrap_or_default();
            drop(guard);
            let db_paths: std::collections::HashSet<String> =
                entries.iter().map(|(_, p)| p.clone()).collect();

            let mut disk_files: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for sub in &["download_VO", "download_VF"] {
                let base = dl_root.join(sub);
                if !base.exists() {
                    continue;
                }
                for entry in walk_dir(&base) {
                    if entry.extension().and_then(|s| s.to_str()) == Some("mp4") {
                        disk_files.insert(entry.to_string_lossy().into_owned());
                    }
                }
            }

            let on_disk_not_in_db: Vec<_> =
                disk_files.iter().filter(|p| !db_paths.contains(*p)).collect();
            let in_db_not_on_disk: Vec<_> = db_paths
                .iter()
                .filter(|p| !disk_files.contains(*p))
                .collect();

            applog::log_event(
                applog::LogSource::App,
                applog::LogLevel::Info,
                format!(
                    "Scan orphelins — {} fichiers sur disque, {} entrées DB, {} fichiers sans entrée, {} entrées sans fichier",
                    disk_files.len(),
                    db_paths.len(),
                    on_disk_not_in_db.len(),
                    in_db_not_on_disk.len(),
                ),
            );
            for p in on_disk_not_in_db.iter().take(20) {
                applog::log_event(
                    applog::LogSource::App,
                    applog::LogLevel::Warn,
                    format!("Disque sans DB: {}", p),
                );
            }
            for p in in_db_not_on_disk.iter().take(20) {
                applog::log_event(
                    applog::LogSource::App,
                    applog::LogLevel::Warn,
                    format!("DB sans disque: {}", p),
                );
            }
        });
    }
}

fn section_header(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(14.0)
            .color(Color32::from_rgb(189, 147, 249))
            .strong(),
    );
    ui.add_space(4.0);
}

fn walk_dir(base: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&p) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                out.push(path);
            }
        }
    }
    out
}

async fn try_anikuro(
    provider: &str,
    anime_title: &str,
    alt_titles: &[String],
    episode_number: usize,
    prefer_dub: bool,
) -> Result<String, String> {
    let client = anikuro::AnikuroClient::new().map_err(|e| e.to_string())?;
    let alts: Vec<&str> = alt_titles.iter().map(|s| s.as_str()).collect();
    let (url, _referer) = client
        .find_stream(provider, anime_title, &alts, episode_number, prefer_dub)
        .await
        .map_err(|e| e.to_string())?;
    Ok(url)
}

async fn try_consumet(
    base_url: &str,
    provider: &str,
    anime_title: &str,
    alt_titles: &[String],
    episode_number: usize,
) -> Result<String, String> {
    let client = consumet::ConsumetClient::new(base_url).map_err(|e| e.to_string())?;
    let alts: Vec<&str> = alt_titles.iter().map(|s| s.as_str()).collect();
    let (url, _is_m3u8, _referer) = client
        .find_episode_url(provider, anime_title, &alts, episode_number)
        .await
        .map_err(|e| e.to_string())?;
    Ok(url)
}

fn apply_va_cached_to_anime(
    display: &mut AnimeDisplay,
    cached: &voiranime::VaCachedEpisodes,
    va_episode_urls: &Arc<StdMutex<HashMap<(u64, usize, usize), String>>>,
    va_episode_sources: &Arc<StdMutex<HashMap<(u64, usize, usize), Vec<voiranime::VaSource>>>>,
) {
    let anime_id_bits = display.anime.id.to_bits();
    let mut url_map = va_episode_urls.lock().unwrap();
    url_map.retain(|(a, _, _), _| *a != anime_id_bits);

    if !cached.description.is_empty() {
        display.anime.description = cached.description.clone();
    }
    if !cached.title.is_empty() && display.anime.title.is_empty() {
        display.anime.title = cached.title.clone();
    }
    if let Some(img) = cached.image.clone() {
        if display.anime.affiche.is_empty() {
            display.anime.affiche = img.clone();
        }
        display.anime.affiche_small = Some(img);
    }
    if let Some(y) = cached.year.clone() {
        if display.anime.start_date.is_empty() {
            display.anime.start_date = y;
        }
    }
    if let Some(st) = cached.status.clone() {
        if display.anime.status.is_empty() {
            display.anime.status = st;
        }
    }
    if let Some(sc) = cached.score {
        if display.anime.note.is_empty() {
            display.anime.note = format!("{:.1}", sc);
        }
    }
    if !cached.genres.is_empty() && display.anime.themes.is_empty() {
        display.anime.themes = cached.genres.clone();
    }

    let mut count_vo = 0usize;
    let mut count_vf = 0usize;
    for e in &cached.episodes {
        if e.lang == "vf" {
            count_vf += 1;
        } else {
            count_vo += 1;
        }
    }
    let want_vo = count_vo > 0;
    let primary_lang = if want_vo { "vostfr" } else { "vf" };
    let primary: Vec<&voiranime::VaEpisode> = cached
        .episodes
        .iter()
        .filter(|e| {
            if want_vo {
                e.lang != "vf"
            } else {
                e.lang == "vf"
            }
        })
        .collect();

    let has_vo_now = count_vo > 0 || (count_vo == 0 && count_vf == 0);
    let has_vf_now = count_vf > 0;
    let mut src_map = va_episode_sources.lock().unwrap();
    src_map.retain(|(a, _, _), _| *a != anime_id_bits);
    let mut saison = crate::animes_api::Saison::default();
    saison.title = "Saison 1".to_string();
    for (e_idx, e) in primary.iter().enumerate() {
        let mut ep = crate::animes_api::Episode::default();
        ep.title = if e.title.is_empty() {
            format!("Épisode {}", e_idx + 1)
        } else {
            e.title.clone()
        };
        let host_names: Vec<String> = if !e.sources.is_empty() {
            e.sources.iter().map(|s| s.host.clone()).collect()
        } else {
            vec!["voiranime".to_string()]
        };
        if e.lang == "vf" {
            ep.lang.vf.lecteurs = host_names.clone();
        } else {
            ep.lang.vo.lecteurs = host_names.clone();
        }
        saison.episodes.push(ep);
        url_map.insert((anime_id_bits, 0, e_idx), e.url.clone());
        if !e.sources.is_empty() {
            src_map.insert((anime_id_bits, 0, e_idx), e.sources.clone());
        }
    }
    display.anime.saisons = vec![saison];
    display.has_vo = has_vo_now;
    display.has_vf = has_vf_now;
    display.va_loaded_episodes = true;
    let _ = primary_lang;
}

async fn run_va_sync(db: Arc<AsyncMutex<Database>>) -> SyncOutcome {
    let client = match voiranime::VaClient::new() {
        Ok(c) => c,
        Err(e) => return SyncOutcome::Failure(format!("client voiranime: {}", e)),
    };
    applog::log_event(
        applog::LogSource::App,
        applog::LogLevel::Info,
        "voiranime sync début".to_string(),
    );
    let mut saved = 0usize;
    let mut errors = 0usize;
    let mut page: i64 = 1;
    let mut consecutive_empty = 0;
    let img_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36")
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let mut images_cached = 0usize;
    loop {
        applog::log_event(
            applog::LogSource::App,
            applog::LogLevel::Info,
            format!("voiranime browse page={}", page),
        );
        let batch = match client.browse_page(page).await {
            Ok(b) => b,
            Err(e) => {
                applog::log_event(
                    applog::LogSource::App,
                    applog::LogLevel::Error,
                    format!("voiranime browse page={} FAIL: {}", page, e),
                );
                errors += 1;
                if errors > 5 {
                    break;
                }
                page += 1;
                continue;
            }
        };
        applog::log_event(
            applog::LogSource::App,
            applog::LogLevel::Info,
            format!("voiranime page={} batch={}", page, batch.len()),
        );
        if batch.is_empty() {
            consecutive_empty += 1;
            if consecutive_empty >= 2 {
                break;
            }
            page += 1;
            continue;
        }
        consecutive_empty = 0;
        {
            let guard = db.lock().await;
            for s in &batch {
                match guard.save_va_anime(s) {
                    Ok(()) => saved += 1,
                    Err(e) => {
                        errors += 1;
                        if errors <= 3 {
                            applog::log_event(
                                applog::LogSource::App,
                                applog::LogLevel::Error,
                                format!("voiranime save '{}' KO: {}", s.slug, e),
                            );
                        }
                    }
                }
            }
        }
        let img_items: Vec<(String, Option<String>)> = batch
            .iter()
            .filter_map(|s| {
                s.image
                    .clone()
                    .map(|u| (u, Some("https://voir-anime.to/".to_string())))
            })
            .collect();
        let n_imgs = cache_images_batch(&db, &img_client, img_items).await;
        images_cached += n_imgs;
        applog::log_event(
            applog::LogSource::App,
            applog::LogLevel::Info,
            format!("voiranime images batch cached: {} (total {})", n_imgs, images_cached),
        );

        page += 1;
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    }
    applog::log_event(
        applog::LogSource::App,
        applog::LogLevel::Info,
        format!(
            "voiranime list sync fini — {} sauvés, {} erreurs, {} images. Deep enrichissement…",
            saved, errors, images_cached
        ),
    );

    let pending = {
        let guard = db.lock().await;
        guard.load_va_animes_pending_deep().unwrap_or_default()
    };
    let total_pending = pending.len();
    applog::log_event(
        applog::LogSource::App,
        applog::LogLevel::Info,
        format!("voiranime deep — {} animes à enrichir", total_pending),
    );

    let progress = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let progress_err = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let semaphore = Arc::new(tokio::sync::Semaphore::new(3));
    use futures::StreamExt;
    let tasks_stream = futures::stream::iter(pending.into_iter().map(|row| {
        let db = db.clone();
        let sem = semaphore.clone();
        let progress = progress.clone();
        let progress_err = progress_err.clone();
        let img_client = img_client.clone();
        async move {
            let _permit = sem.acquire_owned().await.ok();
            let client = match voiranime::VaClient::new() {
                Ok(c) => c,
                Err(_) => {
                    progress_err.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };
            let detail = match client.anime_detail(&row.slug).await {
                Ok(d) => d,
                Err(e) => {
                    applog::log_event(
                        applog::LogSource::App,
                        applog::LogLevel::Warn,
                        format!("voiranime detail '{}' KO: {}", row.slug, e),
                    );
                    progress_err.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };
            let mut enriched_eps: Vec<voiranime::VaEpisode> = Vec::new();
            for ep in &detail.episodes {
                let sources = client
                    .episode_sources(&ep.url)
                    .await
                    .unwrap_or_default();
                let mut e = ep.clone();
                e.sources = sources;
                enriched_eps.push(e);
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            let cached = voiranime::VaCachedEpisodes {
                episodes: enriched_eps,
                description: detail.description.clone(),
                genres: detail.genres.clone(),
                year: detail.year.clone(),
                status: detail.status.clone(),
                score: detail.score,
                image: detail.image.clone(),
                title: detail.title.clone(),
            };
            if let Some(img) = detail.image.clone() {
                let _ = cache_images_batch(
                    &db,
                    &img_client,
                    vec![(img, Some("https://voir-anime.to/".to_string()))],
                )
                .await;
            }
            let json = match serde_json::to_string(&cached) {
                Ok(s) => s,
                Err(_) => {
                    progress_err.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };
            {
                let guard = db.lock().await;
                let _ = guard.save_va_episodes(&row.slug, &json);
                let _ = guard.mark_va_deep_done(&row.slug);
            }
            let done = progress.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if done % 25 == 0 || done == total_pending {
                applog::log_event(
                    applog::LogSource::App,
                    applog::LogLevel::Info,
                    format!(
                        "voiranime deep progress {}/{} (err: {})",
                        done,
                        total_pending,
                        progress_err.load(std::sync::atomic::Ordering::SeqCst)
                    ),
                );
            }
        }
    }))
    .buffer_unordered(3);
    let _: Vec<()> = tasks_stream.collect().await;

    let deep_done = progress.load(std::sync::atomic::Ordering::SeqCst);
    let deep_err = progress_err.load(std::sync::atomic::Ordering::SeqCst);
    applog::log_event(
        applog::LogSource::App,
        applog::LogLevel::Info,
        format!(
            "voiranime deep fini — {} enrichis, {} erreurs",
            deep_done, deep_err
        ),
    );

    SyncOutcome::Success {
        saved: saved + deep_done,
        total: saved + total_pending,
    }
}

fn apply_us_cached_to_anime(
    display: &mut AnimeDisplay,
    cached: &uniquestream::UsCachedEpisodes,
    us_episode_ids: &Arc<StdMutex<HashMap<(u64, usize, usize), String>>>,
    us_audio_locales: &Arc<StdMutex<HashMap<u64, Vec<String>>>>,
    us_movies: &Arc<StdMutex<std::collections::HashSet<String>>>,
) {
    if cached.is_movie {
        if let Some(cid) = &display.us_content_id {
            us_movies.lock().unwrap().insert(cid.clone());
        }
    } else if let Some(cid) = &display.us_content_id {
        us_movies.lock().unwrap().remove(cid);
    }
    let anime_id_bits = display.anime.id.to_bits();
    us_audio_locales
        .lock()
        .unwrap()
        .insert(anime_id_bits, cached.audio_locales.clone());

    let has_dub = cached
        .audio_locales
        .iter()
        .any(|l| l != "ja-JP" && !l.is_empty());
    display.has_vo = cached.audio_locales.iter().any(|l| l == "ja-JP")
        || cached.audio_locales.is_empty();
    display.has_vf = has_dub;

    let mut saisons: Vec<crate::animes_api::Saison> = Vec::new();
    let mut id_map = us_episode_ids.lock().unwrap();
    id_map.retain(|(a, _, _), _| *a != anime_id_bits);

    let has_vo_now = display.has_vo;
    let has_vf_now = display.has_vf;
    for (s_idx, s) in cached.seasons.iter().enumerate() {
        let mut episodes: Vec<crate::animes_api::Episode> = Vec::new();
        for (e_idx, e) in s.episodes.iter().enumerate() {
            let mut ep = crate::animes_api::Episode::default();
            ep.title = if e.title.is_empty() {
                format!("Épisode {}", (e.episode_number as i64).max(1))
            } else {
                e.title.clone()
            };
            if has_vo_now {
                ep.lang.vo.lecteurs = vec!["uniquestream".to_string()];
            }
            if has_vf_now {
                ep.lang.vf.lecteurs = vec!["uniquestream".to_string()];
            }
            episodes.push(ep);
            id_map.insert(
                (anime_id_bits, s_idx, e_idx),
                e.content_id.clone(),
            );
        }
        let mut saison = crate::animes_api::Saison::default();
        saison.title = if s.title.is_empty() {
            format!("Saison {}", s.season_number.max(1))
        } else {
            s.title.clone()
        };
        saison.episodes = episodes;
        saisons.push(saison);
    }
    display.anime.saisons = saisons;
    display.us_loaded_episodes = true;
}

fn host_from_url(url: &str) -> String {
    let trimmed = url
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let domain = trimmed.split('/').next().unwrap_or(trimmed);
    let parts: Vec<&str> = domain.split('.').collect();
    if parts.len() >= 2 {
        parts[parts.len() - 2].to_string()
    } else {
        domain.to_string()
    }
}

fn us_id_from(content_id: &str) -> f64 {
    let mut h: u64 = 14695981039346656037u64;
    for b in content_id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211u64);
    }
    let masked = (h & ((1u64 << 52) - 1)) as f64;
    -1.0 - masked
}

fn build_filename(
    format: &str,
    anime_name: &str,
    season_idx: usize,
    ep_idx: usize,
    episode_name: &str,
    lang: &str,
) -> String {
    if format == "plex" {
        format!(
            "{} - S{:02}E{:02} - {} [{}].mp4",
            sanitize_path_segment(anime_name),
            season_idx + 1,
            ep_idx + 1,
            sanitize_path_segment(episode_name),
            lang.to_uppercase()
        )
    } else {
        format!("{}.mp4", sanitize_path_segment(episode_name))
    }
}

fn format_log_line(e: &applog::LogEntry) -> String {
    let ts = format_timestamp(e.ts);
    format!("{}  [{:7}/{:5}] {}", ts, e.source.short(), e.level.short(), e.message)
}

fn format_timestamp(ts: std::time::SystemTime) -> String {
    let secs = ts
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

fn render_log_row(ui: &mut egui::Ui, entry: &applog::LogEntry) {
    let level_color = match entry.level {
        applog::LogLevel::Info => Color32::from_rgb(139, 233, 253),
        applog::LogLevel::Warn => Color32::from_rgb(241, 196, 15),
        applog::LogLevel::Error => Color32::from_rgb(255, 85, 85),
    };
    let source_color = match entry.source {
        applog::LogSource::App => Color32::from_rgb(189, 147, 249),
        applog::LogSource::Sidecar => Color32::from_rgb(139, 233, 253),
        applog::LogSource::Python => Color32::from_rgb(80, 250, 123),
    };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format_timestamp(entry.ts))
                .size(10.0)
                .monospace()
                .color(Color32::from_rgb(120, 120, 130)),
        );
        egui::Frame::NONE
            .fill(source_color)
            .corner_radius(3.0)
            .inner_margin(egui::vec2(4.0, 1.0))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(entry.source.short())
                        .size(9.0)
                        .color(Color32::BLACK)
                        .strong(),
                );
            });
        ui.label(
            RichText::new(entry.level.short())
                .size(10.0)
                .monospace()
                .color(level_color)
                .strong(),
        );
        ui.label(
            RichText::new(&entry.message)
                .size(11.0)
                .monospace()
                .color(Color32::from_rgb(220, 220, 230)),
        );
    });
}

fn stat_card(ui: &mut egui::Ui, label: &str, value: &str, accent: Color32) {
    egui::Frame::NONE
        .fill(Color32::from_rgb(50, 52, 64))
        .corner_radius(6.0)
        .inner_margin(egui::vec2(12.0, 8.0))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(label)
                        .size(10.0)
                        .color(Color32::from_rgb(150, 150, 160)),
                );
                ui.label(RichText::new(value).size(20.0).color(accent).strong());
            });
        });
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([1000.0, 700.0])
            .with_title("Anime Downloader"),
        ..Default::default()
    };

    eframe::run_native(
        "Anime Downloader",
        options,
        Box::new(|cc| Ok(Box::new(AnimeDownloaderApp::new(cc)))),
    )
}
