use crate::animes_api::{FullAnimeslist, Root2};
use crate::downloader::{DownloadManager, DownloadTask};
use crate::url_fetcher::UrlFetcher;
use eframe::egui;
use egui::{Color32, ColorImage, RichText, Vec2};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock, RwLockWriteGuard};
use tokio::runtime::Runtime;

mod animes_api;
mod downloader;
mod url_fetcher;
mod worker;

#[derive(Debug, Clone, PartialEq)]
pub enum DownloadStatus {
    NotDownloaded,
    Downloading(f32),
    Downloaded,
}

#[derive(Debug, Clone)]
pub struct AnimeDisplay {
    pub anime: Root2,
    pub download_status: DownloadStatus,
    pub has_vo: bool,
    pub has_vf: bool,
    pub image_loaded: bool,
    pub expanded: bool,
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
        }
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
            "CREATE INDEX IF NOT EXISTS idx_anime_title ON animes(title)",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_anime_updated ON animes(updated_date)",
            [],
        )?;

        Ok(())
    }

    fn save_or_update_anime(&self, anime: &Root2) -> Result<(), rusqlite::Error> {
        let (has_vo, has_vf) = AnimeDisplay::check_languages(anime);
        let json_data = serde_json::to_string(anime).unwrap_or_default();

        let exists: bool = self.conn.query_row(
            "SELECT COUNT(*) FROM animes WHERE id = ?1",
            params![anime.id],
            |row| {
                let count: i64 = row.get(0)?;
                Ok(count > 0)
            },
        )?;

        if exists {
            self.conn.execute(
                "UPDATE animes SET
                    json_data = ?2,
                    has_vo = ?3,
                    has_vf = ?4,
                    title = ?5,
                    title_o = ?6,
                    note = ?7,
                    status = ?8,
                    nsfw = ?9,
                    updated_date = ?10,
                    updated_date_vf = ?11,
                    updated_at = strftime('%s', 'now')
                WHERE id = ?1",
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
        } else {
            self.conn.execute(
                "INSERT INTO animes (
                    id, json_data, has_vo, has_vf, title, title_o,
                    note, status, nsfw, updated_date, updated_date_vf
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
        }

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
    filtered_animes: Vec<AnimeDisplay>,
    search_query: String,
    lang_filter: LangFilter,
    images: HashMap<String, egui::TextureHandle>,
    db: Arc<Mutex<Database>>,
    runtime: Runtime,
    is_syncing: bool,
    sync_status: String,
    manager: Arc<DownloadManager>,
    tasks: Arc<RwLock<Vec<DownloadTask>>>,
    url_input: String,
    output_input: String,
}

#[derive(Debug, Clone, PartialEq)]
enum LangFilter {
    All,
    VO,
    VF,
    Both,
}

impl AnimeDownloaderApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals.window_corner_radius = 10.0.into();
        style.visuals.widgets.noninteractive.corner_radius = 8.0.into();
        cc.egui_ctx.set_style(style);

        let db = Arc::new(Mutex::new(
            Database::new().expect("Failed to create database"),
        ));

        let animes = if let Ok(db_lock) = db.lock() {
            db_lock
                .load_animes()
                .unwrap_or_default()
                .into_iter()
                .map(AnimeDisplay::new)
                .collect()
        } else {
            vec![]
        };

        let filtered_animes = animes.clone();

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (manager, mut updates) =
            runtime.block_on(async { DownloadManager::new(false, 3).await.unwrap() });

        let manager = Arc::new(manager);
        let tasks = Arc::new(RwLock::new(Vec::<DownloadTask>::new()));

        let tasks_clone = tasks.clone();
        runtime.spawn(async move {
            while let Some(task) = updates.recv().await {
                let mut tasks: RwLockWriteGuard<Vec<DownloadTask>> =
                    tasks_clone.write().expect("Failed to lock tasks");
                if let Some(t) = tasks.iter_mut().find(|t| t.id == task.id) {
                    *t = task;
                } else {
                    tasks.push(task);
                }
            }
        });

        Self {
            animes,
            filtered_animes,
            search_query: String::new(),
            lang_filter: LangFilter::All,
            images: HashMap::new(),
            db,
            runtime,
            is_syncing: false,
            sync_status: String::new(),
            manager,
            tasks,
            url_input: String::new(),
            output_input: String::new(),
        }
    }

    fn sync_from_api(&mut self, ctx: egui::Context) {
        if self.is_syncing {
            return;
        }

        self.is_syncing = true;
        self.sync_status = "🔄 Synchronisation en cours...".to_string();

        let db = Arc::clone(&self.db);
        let ctx_clone = ctx.clone();

        self.runtime.spawn(async move {
            /*

            GET_LECTEUR: (e, t, s, a, r) => "anime/".concat(e, "/").concat(t, "/").concat(s, "/").concat(a, "/").concat(r),

            async function eC(e) {
                let {animeId: t, saisonIndex: l, episodeIndex: s, lang: a, lecteurIndex: r} = e,
                    i = await (0, B.A)({
                        endpoint: H.S.GET_LECTEUR(t, l, s, a, r)
                    }, {
                        method: "GET",
                        headers: {
                            "Content-Type": "application/json"
                        }
                    });
                if (!i.ok)
                    throw Error("Failed to get lecteur URL");
                return {
                    url: await i.text()
                }
            }
                 */
            match reqwest::get("https://api.franime.fr/api/animes").await {
                Ok(response) => match response.text().await {
                    Ok(json_text) => match serde_json::from_str::<FullAnimeslist>(&json_text) {
                        Ok(animes) => {
                            let total = animes.len();
                            let mut saved = 0;

                            for anime in animes {
                                {
                                    if let Ok(db_lock) = db.lock() {
                                        if db_lock.save_or_update_anime(&anime).is_ok() {
                                            saved += 1;
                                        }
                                    }
                                }

                                println!("✅ Anime {saved}/{total} progress");

                                let image_url = anime
                                    .affiche_small
                                    .as_ref()
                                    .unwrap_or(&anime.affiche)
                                    .clone();

                                let needs_download = {
                                    if let Ok(db_lock) = db.lock() {
                                        db_lock.get_image(&image_url).ok().flatten().is_none()
                                    } else {
                                        false
                                    }
                                };

                                if needs_download {
                                    if let Ok(img_response) = reqwest::get(&image_url).await {
                                        if let Ok(img_bytes) = img_response.bytes().await {
                                            if let Ok(img) = image::load_from_memory(&img_bytes) {
                                                let rgba = img.to_rgba8();
                                                let (width, height) = rgba.dimensions();
                                                let img_bytes_vec = img_bytes.to_vec();

                                                if let Ok(db_lock) = db.lock() {
                                                    let _ = db_lock.save_image(
                                                        &image_url,
                                                        &img_bytes_vec,
                                                        width,
                                                        height,
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            println!(
                                "✅ Synchronisation terminée: {}/{} animes sauvegardés",
                                saved, total
                            );
                        }
                        Err(e) => eprintln!("❌ Erreur parsing JSON: {}", e),
                    },
                    Err(e) => eprintln!("❌ Erreur lecture réponse: {}", e),
                },
                Err(e) => eprintln!("❌ Erreur requête API: {}", e),
            }

            ctx_clone.request_repaint();
        });
    }

    fn reload_from_db(&mut self) {
        let loaded_animes = {
            if let Ok(db_lock) = self.db.lock() {
                db_lock.load_animes().unwrap_or_default()
            } else {
                vec![]
            }
        };

        self.animes = loaded_animes.into_iter().map(AnimeDisplay::new).collect();

        self.filter_animes();
        self.is_syncing = false;
        self.sync_status = format!("✅ {} animes chargés", self.animes.len());
    }

    fn load_image_from_db(
        &mut self,
        url: &str,
        ctx: &egui::Context,
    ) -> Option<egui::TextureHandle> {
        if self.images.contains_key(url) {
            return self.images.get(url).cloned();
        }

        if let Ok(db_lock) = self.db.lock() {
            if let Ok(Some(image_data)) = db_lock.get_image(url) {
                if let Ok(img) = image::load_from_memory(&image_data) {
                    let rgba = img.to_rgba8();
                    let (width, height) = rgba.dimensions();
                    let pixels = rgba.into_raw();

                    let color_image = ColorImage::from_rgba_unmultiplied(
                        [width as usize, height as usize],
                        &pixels,
                    );

                    let texture =
                        ctx.load_texture(url, color_image, egui::TextureOptions::default());

                    self.images.insert(url.to_string(), texture.clone());
                    return Some(texture);
                }
            }
        }

        None
    }

    fn levenshtein_distance(s1: &str, s2: &str) -> usize {
        let s1 = s1.to_lowercase();
        let s2 = s2.to_lowercase();
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

    fn matches_search(&self, anime: &AnimeDisplay, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }

        let query_lower = query.to_lowercase();

        if anime.anime.title.to_lowercase().contains(&query_lower) {
            return true;
        }

        if anime.anime.title_o.to_lowercase().contains(&query_lower) {
            return true;
        }

        if let Some(ref ja_jp) = anime.anime.titles.ja_jp {
            if ja_jp.to_lowercase().contains(&query_lower) {
                return true;
            }
        }

        let distance = Self::levenshtein_distance(&query_lower, &anime.anime.title.to_lowercase());
        let threshold = (query.len() as f32 * 0.4) as usize;

        distance <= threshold
    }

    fn filter_animes(&mut self) {
        self.filtered_animes = self
            .animes
            .iter()
            .filter(|anime| {
                let lang_match = match self.lang_filter {
                    LangFilter::All => true,
                    LangFilter::VO => anime.has_vo && !anime.has_vf,
                    LangFilter::VF => anime.has_vf && !anime.has_vo,
                    LangFilter::Both => anime.has_vo && anime.has_vf,
                };

                if !lang_match {
                    return false;
                }

                self.matches_search(anime, &self.search_query)
            })
            .cloned()
            .collect();
    }

    fn render_anime_card(&mut self, ui: &mut egui::Ui, anime: &AnimeDisplay, ctx: &egui::Context) {
        let anime_id = anime.anime.id;

        egui::Frame::NONE
            .fill(Color32::from_rgb(40, 42, 54))
            .corner_radius(12.0)
            .inner_margin(15.0)
            .stroke(egui::Stroke::new(1.0, Color32::from_rgb(60, 62, 74)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        let img_size = Vec2::new(140.0, 200.0);

                        let image_url = anime.anime.affiche_small.as_ref()
                            .unwrap_or(&anime.anime.affiche);

                        if let Some(texture) = self.load_image_from_db(image_url, ctx) {
                            ui.image((texture.id(), img_size));
                        } else {
                            let (rect, _) = ui.allocate_exact_size(img_size, egui::Sense::hover());
                            ui.painter().rect_filled(rect, 8.0, Color32::from_rgb(30, 32, 44));
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "🎬",
                                egui::FontId::proportional(50.0),
                                Color32::from_rgb(100, 100, 110),
                            );
                        }
                    });

                    ui.add_space(20.0);


                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.heading(RichText::new(&anime.anime.title)
                                .size(22.0)
                                .color(Color32::from_rgb(139, 233, 253))
                                .strong());

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let expand_icon = if anime.expanded { "▼" } else { "▶" };
                                let expand_button = egui::Button::new(
                                    RichText::new(expand_icon).size(18.0).color(Color32::WHITE)
                                )
                                    .fill(Color32::from_rgb(68, 71, 90))
                                    .corner_radius(6.0);

                                if ui.add(expand_button).clicked() {
                                    if let Some(anime_display) = self.animes.iter_mut().find(|a| a.anime.id == anime_id) {
                                        anime_display.expanded = !anime_display.expanded;
                                    }
                                    if let Some(anime_display) = self.filtered_animes.iter_mut().find(|a| a.anime.id == anime_id) {
                                        anime_display.expanded = !anime_display.expanded;
                                    }
                                }
                            });
                        });


                        if !anime.anime.title_o.is_empty() && anime.anime.title_o != anime.anime.title {
                            ui.label(RichText::new(&anime.anime.title_o)
                                .size(14.0)
                                .color(Color32::from_rgb(150, 150, 160))
                                .italics());
                        }

                        ui.add_space(10.0);

                        ui.horizontal(|ui| {
                            if !anime.anime.note.is_empty() {
                                ui.label(RichText::new(format!("⭐ {}", anime.anime.note))
                                    .color(Color32::from_rgb(241, 250, 140)));
                            }

                            ui.separator();
                            ui.label(RichText::new(format!("📅 {}", anime.anime.start_date))
                                .color(Color32::from_rgb(150, 150, 160)));

                            ui.separator();
                            ui.label(RichText::new(format!("📺 {} eps", anime.total_episodes()))
                                .color(Color32::from_rgb(150, 150, 160)));

                            ui.separator();
                            ui.label(RichText::new(format!("📦 {} saison(s)", anime.anime.saisons.len()))
                                .color(Color32::from_rgb(150, 150, 160)));
                        });

                        ui.add_space(8.0);

                        ui.horizontal(|ui| {
                            if anime.has_vo {
                                egui::Frame::NONE
                                    .fill(Color32::from_rgb(80, 250, 123))
                                    .corner_radius(5.0)
                                    .inner_margin(egui::vec2(10.0, 5.0))
                                    .show(ui, |ui| {
                                        ui.label(RichText::new("VO").size(13.0).color(Color32::BLACK).strong());
                                    });
                            }

                            if anime.has_vf {
                                egui::Frame::NONE
                                    .fill(Color32::from_rgb(255, 121, 198))
                                    .corner_radius(5.0)
                                    .inner_margin(egui::vec2(10.0, 5.0))
                                    .show(ui, |ui| {
                                        ui.label(RichText::new("VF").size(13.0).color(Color32::BLACK).strong());
                                    });
                            }

                            if anime.anime.nsfw {
                                egui::Frame::NONE
                                    .fill(Color32::from_rgb(255, 85, 85))
                                    .corner_radius(5.0)
                                    .inner_margin(egui::vec2(8.0, 5.0))
                                    .show(ui, |ui| {
                                        ui.label(RichText::new("🔞 NSFW").size(12.0).color(Color32::WHITE).strong());
                                    });
                            }
                        });

                        ui.add_space(8.0);

                        if !anime.anime.themes.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                for theme in anime.anime.themes.iter().take(5) {
                                    egui::Frame::NONE
                                        .fill(Color32::from_rgb(68, 71, 90))
                                        .corner_radius(5.0)
                                        .inner_margin(egui::vec2(8.0, 4.0))
                                        .show(ui, |ui| {
                                            ui.label(RichText::new(theme).size(11.0).color(Color32::from_rgb(189, 147, 249)));
                                        });
                                }
                            });
                        }

                        ui.add_space(10.0);


                        let description = if anime.anime.description.len() > 200 {
                            let mut truncate_at = 200;
                            while truncate_at > 0 && !anime.anime.description.is_char_boundary(truncate_at) {
                                truncate_at -= 1;
                            }
                            if truncate_at > 0 {
                                format!("{}...", &anime.anime.description[..truncate_at])
                            } else {
                                anime.anime.description.clone()
                            }
                        } else {
                            anime.anime.description.clone()
                        };

                        ui.label(RichText::new(description).size(13.0).color(Color32::from_rgb(200, 200, 210)));

                        ui.add_space(12.0);


                        ui.horizontal(|ui| {
                            let download_all_button = egui::Button::new(
                                RichText::new("⬇ Télécharger tout")
                                    .size(14.0)
                                    .color(Color32::WHITE)
                                    .strong()
                            )
                                .fill(Color32::from_rgb(139, 233, 253))
                                .corner_radius(8.0)
                                .min_size(Vec2::new(180.0, 35.0));

                            if ui.add(download_all_button).clicked() {
                                println!("📥 Téléchargement complet de l'anime {} démarré", if anime.anime.title.is_empty() { anime.anime.title_o.clone() } else { anime.anime.title.clone() });
                            }


                            let open_button = egui::Button::new(
                                RichText::new("🔗 Ouvrir la page")
                                    .size(14.0)
                                    .color(Color32::WHITE)
                                    .strong()
                            )
                                .fill(Color32::from_rgb(189, 147, 249))
                                .corner_radius(8.0)
                                .min_size(Vec2::new(160.0, 35.0));

                            if ui.add(open_button).clicked() {
                                if let Err(e) = open::that(&anime.anime.source_url.replace("/api/edge", "")) {
                                    eprintln!("❌ Erreur ouverture URL: {}", e);
                                } else {
                                    println!("✅ Ouverture de: {}", anime.anime.source_url);
                                }
                            }
                        });
                    });
                });


                if anime.expanded {
                    ui.add_space(15.0);
                    ui.separator();
                    ui.add_space(10.0);

                    if anime.anime.saisons.is_empty() {
                        ui.label(RichText::new("Aucune saison disponible")
                            .size(13.0)
                            .color(Color32::from_rgb(150, 150, 160))
                            .italics());
                    } else {
                        for (season_idx, saison) in anime.anime.saisons.iter().enumerate() {
                            egui::Frame::NONE
                                .fill(Color32::from_rgb(50, 52, 64))
                                .corner_radius(8.0)
                                .inner_margin(12.0)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(format!("📦 {}", saison.title))
                                            .size(16.0)
                                            .color(Color32::from_rgb(139, 233, 253))
                                            .strong());

                                        ui.label(RichText::new(format!("({} épisodes)", saison.episodes.len()))
                                            .size(13.0)
                                            .color(Color32::from_rgb(150, 150, 160)));
                                    });

                                    ui.add_space(8.0);


                                    for (ep_idx, episode) in saison.episodes.iter().enumerate() {
                                        ui.horizontal(|ui| {
                                            ui.add_space(10.0);


                                            ui.label(RichText::new(format!("Ep {}", ep_idx + 1))
                                                .size(12.0)
                                                .color(Color32::from_rgb(241, 250, 140))
                                                .strong());

                                            ui.label(RichText::new(&episode.title)
                                                .size(12.0)
                                                .color(Color32::from_rgb(200, 200, 210)));

                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                if !episode.lang.vf.lecteurs.is_empty() {
                                                    for (idx, name) in episode.lang.vf.lecteurs.clone().into_iter().enumerate() {
                                                        if name == "hd" || name == "TELECHARGEMENT UNIQUE" { continue; }
                                                        let vf_button = egui::Button::new(
                                                            RichText::new(format!("⬇ VF ({})", name)).size(11.0).color(Color32::BLACK).strong()
                                                        )
                                                            .fill(Color32::from_rgb(255, 121, 198))
                                                            .corner_radius(5.0)
                                                            .min_size(Vec2::new(55.0, 22.0));

                                                        if ui.add(vf_button).clicked() {
                                                            println!("📥 Téléchargement VF - S{}E{} de '{}' ({} lecteurs)",
                                                                     season_idx + 1, ep_idx + 1, anime.anime.saisons[season_idx].episodes[ep_idx].title,
                                                                     episode.lang.vf.lecteurs.len());
                                                            println!("   Lecteurs VF: {:?}", episode.lang.vf.lecteurs);


                                                            let fetcher = UrlFetcher::new();
                                                            let url = fetcher.fetch_video_url(
                                                                anime.anime.id as u64,
                                                                season_idx as u64,
                                                                ep_idx as u64,
                                                                "vf",
                                                                idx as u64
                                                            );

                                                            if let Ok(url) = url {
                                                                let manager = self.manager.clone();

                                                                let season = anime.anime.saisons[season_idx].title.clone();
                                                                let episode = anime.anime.saisons[season_idx].episodes[ep_idx].title.clone();
                                                                let anime_name = anime.anime.title_o.clone();

                                                                self.runtime.spawn(async move {
                                                                    manager.add_download(url, format!("download_VF/{}/{}", anime_name, season), format!("{}.mp4", episode)).await;
                                                                });

                                                                //println!("   URL VF: {}", url);

                                                            }
                                                        }
                                                    }
                                                }


                                                if !episode.lang.vo.lecteurs.is_empty() {
                                                    for (idx, name) in episode.lang.vo.lecteurs.clone().into_iter().enumerate() {
                                                        if name == "hd" || name == "TELECHARGEMENT UNIQUE" { continue; }
                                                        let vo_button = egui::Button::new(
                                                            RichText::new(format!("⬇ VO ({})", name)).size(11.0).color(Color32::BLACK).strong()
                                                        )
                                                            .fill(Color32::from_rgb(80, 250, 123))
                                                            .corner_radius(5.0)
                                                            .min_size(Vec2::new(55.0, 22.0));

                                                        if ui.add(vo_button).clicked() {
                                                            println!("📥 Téléchargement VO - S{}E{} de '{}' ({} lecteurs)",
                                                                     season_idx + 1, ep_idx + 1, anime.anime.saisons[season_idx].episodes[ep_idx].title,
                                                                     episode.lang.vo.lecteurs.len());
                                                            println!("   Lecteurs VO: {:?}", episode.lang.vo.lecteurs);


                                                            let fetcher = UrlFetcher::new();
                                                            let url = fetcher.fetch_video_url(
                                                                anime.anime.id as u64,
                                                                season_idx as u64,
                                                                ep_idx as u64,
                                                                "vo",
                                                                idx as u64
                                                            );

                                                            if let Ok(url) = url {
                                                                let manager = self.manager.clone();
                                                                let season = anime.anime.saisons[season_idx].title.clone();
                                                                let episode = anime.anime.saisons[season_idx].episodes[ep_idx].title.clone();
                                                                let anime_name = anime.anime.title_o.clone();

                                                                self.runtime.spawn(async move {
                                                                    manager.add_download(url, format!("download_VO/{}/{}", anime_name, season), format!("{}.mp4", episode)).await;
                                                                });
                                                            }
                                                        }
                                                    }
                                                }


                                                if episode.lang.vo.lecteurs.is_empty() && episode.lang.vf.lecteurs.is_empty() {
                                                    ui.label(RichText::new("❌ Indisponible")
                                                        .size(11.0)
                                                        .color(Color32::from_rgb(150, 150, 160))
                                                        .italics());
                                                }
                                            });
                                        });

                                        if ep_idx < saison.episodes.len() - 1 {
                                            ui.add_space(4.0);
                                        }
                                    }
                                });

                            ui.add_space(8.0);
                        }
                    }
                }
            });
    }
}

impl eframe::App for AnimeDownloaderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::from_rgb(40, 42, 54)))
            .show(ctx, |ui| {
                ui.add_space(20.0);

                egui::Frame::NONE
                    .fill(Color32::from_rgb(68, 71, 90))
                    .corner_radius(15.0)
                    .inner_margin(25.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.heading(
                                    RichText::new("🎌 Anime Downloader")
                                        .size(40.0)
                                        .color(Color32::from_rgb(189, 147, 249))
                                        .strong(),
                                );
                                ui.label(
                                    RichText::new("Gérez votre collection d'animes")
                                        .size(14.0)
                                        .color(Color32::from_rgb(150, 150, 160)),
                                );
                            });

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let sync_button = egui::Button::new(
                                        RichText::new(if self.is_syncing {
                                            "⏳ Sync..."
                                        } else {
                                            "🔄 Synchroniser API"
                                        })
                                        .size(16.0)
                                        .color(Color32::WHITE)
                                        .strong(),
                                    )
                                    .fill(if self.is_syncing {
                                        Color32::from_rgb(150, 150, 160)
                                    } else {
                                        Color32::from_rgb(80, 250, 123)
                                    })
                                    .corner_radius(8.0)
                                    .min_size(Vec2::new(180.0, 45.0));

                                    if ui.add_enabled(!self.is_syncing, sync_button).clicked() {
                                        self.sync_from_api(ctx.clone());
                                    }

                                    ui.add_space(10.0);

                                    let reload_button = egui::Button::new(
                                        RichText::new("♻️ Recharger DB")
                                            .size(16.0)
                                            .color(Color32::WHITE)
                                            .strong(),
                                    )
                                    .fill(Color32::from_rgb(139, 233, 253))
                                    .corner_radius(8.0)
                                    .min_size(Vec2::new(160.0, 45.0));

                                    if ui.add(reload_button).clicked() {
                                        self.reload_from_db();
                                    }
                                },
                            );
                        });

                        if !self.sync_status.is_empty() {
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new(&self.sync_status)
                                    .size(13.0)
                                    .color(Color32::from_rgb(241, 250, 140)),
                            );
                        }
                    });

                ui.add_space(20.0);

                egui::Frame::NONE
                    .fill(Color32::from_rgb(68, 71, 90))
                    .corner_radius(12.0)
                    .inner_margin(20.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("🔍").size(20.0));
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut self.search_query)
                                    .hint_text("Rechercher par titre (Levenshtein activé)...")
                                    .desired_width(500.0),
                            );

                            if response.changed() {
                                self.filter_animes();
                            }

                            ui.add_space(20.0);

                            ui.label(
                                RichText::new("Langue:")
                                    .size(14.0)
                                    .color(Color32::from_rgb(200, 200, 210)),
                            );

                            let filters = [
                                (LangFilter::All, "Toutes", Color32::from_rgb(100, 100, 110)),
                                (LangFilter::VO, "VO", Color32::from_rgb(80, 250, 123)),
                                (LangFilter::VF, "VF", Color32::from_rgb(255, 121, 198)),
                                (
                                    LangFilter::Both,
                                    "VO + VF",
                                    Color32::from_rgb(139, 233, 253),
                                ),
                            ];

                            for (filter, label, color) in filters {
                                let is_selected = self.lang_filter == filter;
                                let button = egui::Button::new(
                                    RichText::new(label)
                                        .size(13.0)
                                        .color(if is_selected {
                                            Color32::BLACK
                                        } else {
                                            Color32::WHITE
                                        })
                                        .strong(),
                                )
                                .fill(if is_selected {
                                    color
                                } else {
                                    Color32::from_rgb(50, 52, 64)
                                })
                                .corner_radius(6.0);

                                if ui.add(button).clicked() {
                                    self.lang_filter = filter;
                                    self.filter_animes();
                                }
                            }
                        });

                        ui.add_space(10.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "📊 {} anime(s) trouvé(s)",
                                    self.filtered_animes.len()
                                ))
                                .size(13.0)
                                .color(Color32::from_rgb(139, 233, 253)),
                            );

                            ui.separator();

                            ui.label(
                                RichText::new(format!(
                                    "💾 {} total dans la base",
                                    self.animes.len()
                                ))
                                .size(13.0)
                                .color(Color32::from_rgb(150, 150, 160)),
                            );
                        });
                    });

                ui.add_space(20.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        if self.filtered_animes.is_empty() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(100.0);
                                ui.label(
                                    RichText::new("😔 Aucun anime trouvé")
                                        .size(24.0)
                                        .color(Color32::from_rgb(150, 150, 160)),
                                );
                                ui.add_space(10.0);
                                ui.label(
                                    RichText::new(
                                        "Cliquez sur 'Synchroniser API' pour charger les animes",
                                    )
                                    .size(14.0)
                                    .color(Color32::from_rgb(120, 120, 130)),
                                );
                            });
                        } else {
                            for anime in &self.filtered_animes.clone() {
                                self.render_anime_card(ui, anime, ctx);
                                ui.add_space(15.0);
                            }
                        }
                    });
            });
    }
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
