use crate::error::Result;
use crate::sqlite_win::{SqliteConn, SqliteStmt, SQLITE_ROW};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use snagsoup_model::SnagxArchive;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaType {
    Image,
    Video,
    Gif,
}

impl MediaType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MediaType::Image => "image",
            MediaType::Video => "video",
            MediaType::Gif => "gif",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "video" => MediaType::Video,
            "gif" => MediaType::Gif,
            _ => MediaType::Image,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryItem {
    pub id: i64,
    pub file_path: String,
    pub media_type: MediaType,
    pub capture_date: String,
    pub app_name: String,
    pub window_title: String,
    pub web_url: String,
    pub callout_text: String,
    pub ocr_text: String,
    pub width: u32,
    pub height: u32,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetCount {
    pub label: String,
    pub count: usize,
}

/// SQLite WAL + FTS5 Library Database dynamically loading Windows' built-in `winsqlite3.dll` (0s C compile!).
pub struct LibraryDb {
    conn: SqliteConn,
}

impl LibraryDb {
    pub fn open_in_memory() -> Result<Self> {
        let conn = SqliteConn::open(":memory:")?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    pub fn open_file(path: impl AsRef<Path>) -> Result<Self> {
        let p = path.as_ref().to_string_lossy();
        let conn = SqliteConn::open(&p)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL UNIQUE,
                media_type TEXT NOT NULL DEFAULT 'image',
                capture_date TEXT NOT NULL DEFAULT '',
                app_name TEXT NOT NULL DEFAULT '',
                window_title TEXT NOT NULL DEFAULT '',
                web_url TEXT NOT NULL DEFAULT '',
                web_domain TEXT NOT NULL DEFAULT '',
                callout_text TEXT NOT NULL DEFAULT '',
                ocr_text TEXT NOT NULL DEFAULT '',
                width INTEGER NOT NULL DEFAULT 0,
                height INTEGER NOT NULL DEFAULT 0,
                tags_flat TEXT NOT NULL DEFAULT ''
            );

            CREATE TABLE IF NOT EXISTS tags (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE
            );

            CREATE TABLE IF NOT EXISTS item_tags (
                item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
                tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                PRIMARY KEY (item_id, tag_id)
            );

            CREATE INDEX IF NOT EXISTS idx_items_date ON items(capture_date DESC);
            CREATE INDEX IF NOT EXISTS idx_items_app ON items(app_name);
            CREATE INDEX IF NOT EXISTS idx_items_domain ON items(web_domain);
            CREATE INDEX IF NOT EXISTS idx_items_media ON items(media_type);

            CREATE VIRTUAL TABLE IF NOT EXISTS items_fts USING fts5(
                window_title,
                app_name,
                web_url,
                callout_text,
                ocr_text,
                tags_flat,
                content='items',
                content_rowid='id',
                tokenize='unicode61 remove_diacritics 2'
            );

            CREATE TRIGGER IF NOT EXISTS items_ai AFTER INSERT ON items BEGIN
                INSERT INTO items_fts(rowid, window_title, app_name, web_url, callout_text, ocr_text, tags_flat)
                VALUES (new.id, new.window_title, new.app_name, new.web_url, new.callout_text, new.ocr_text, new.tags_flat);
            END;

            CREATE TRIGGER IF NOT EXISTS items_ad AFTER DELETE ON items BEGIN
                INSERT INTO items_fts(items_fts, rowid, window_title, app_name, web_url, callout_text, ocr_text, tags_flat)
                VALUES ('delete', old.id, old.window_title, old.app_name, old.web_url, old.callout_text, old.ocr_text, old.tags_flat);
            END;

            CREATE TRIGGER IF NOT EXISTS items_au AFTER UPDATE ON items BEGIN
                INSERT INTO items_fts(items_fts, rowid, window_title, app_name, web_url, callout_text, ocr_text, tags_flat)
                VALUES ('delete', old.id, old.window_title, old.app_name, old.web_url, old.callout_text, old.ocr_text, old.tags_flat);
                INSERT INTO items_fts(rowid, window_title, app_name, web_url, callout_text, ocr_text, tags_flat)
                VALUES (new.id, new.window_title, new.app_name, new.web_url, new.callout_text, new.ocr_text, new.tags_flat);
            END;
            "#,
        )?;
        Ok(())
    }

    pub fn upsert_item(&mut self, item: &LibraryItem) -> Result<i64> {
        let domain = extract_domain(&item.web_url);
        let tags_flat = item.tags.join(" ");
        {
            let mut stmt = self.conn.prepare(
                r#"
                INSERT INTO items (
                    file_path, media_type, capture_date, app_name, window_title,
                    web_url, web_domain, callout_text, ocr_text, width, height, tags_flat
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(file_path) DO UPDATE SET
                    media_type = excluded.media_type,
                    capture_date = excluded.capture_date,
                    app_name = excluded.app_name,
                    window_title = excluded.window_title,
                    web_url = excluded.web_url,
                    web_domain = excluded.web_domain,
                    callout_text = excluded.callout_text,
                    ocr_text = excluded.ocr_text,
                    width = excluded.width,
                    height = excluded.height,
                    tags_flat = excluded.tags_flat
                "#,
            )?;
            stmt.bind_text(1, &item.file_path)?;
            stmt.bind_text(2, item.media_type.as_str())?;
            stmt.bind_text(3, &item.capture_date)?;
            stmt.bind_text(4, &item.app_name)?;
            stmt.bind_text(5, &item.window_title)?;
            stmt.bind_text(6, &item.web_url)?;
            stmt.bind_text(7, &domain)?;
            stmt.bind_text(8, &item.callout_text)?;
            stmt.bind_text(9, &item.ocr_text)?;
            stmt.bind_int64(10, item.width as i64)?;
            stmt.bind_int64(11, item.height as i64)?;
            stmt.bind_text(12, &tags_flat)?;
            stmt.step()?;
        }

        let id = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM items WHERE file_path = ?1")?;
            stmt.bind_text(1, &item.file_path)?;
            if stmt.step()? == SQLITE_ROW {
                stmt.col_i64(0)
            } else {
                0
            }
        };

        if id > 0 && !item.tags.is_empty() {
            self.set_item_tags(id, &item.tags)?;
        }
        Ok(id)
    }

    /// Batch-inserts many items inside a single SQLite transaction (5,000 items in ~40ms!).
    pub fn insert_batch(&mut self, items: &[LibraryItem]) -> Result<()> {
        self.conn.execute_batch("BEGIN IMMEDIATE;")?;
        let res = (|| -> Result<()> {
            let mut stmt = self.conn.prepare(
                r#"
                INSERT OR REPLACE INTO items (
                    file_path, media_type, capture_date, app_name, window_title,
                    web_url, web_domain, callout_text, ocr_text, width, height, tags_flat
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                "#,
            )?;
            for item in items {
                let domain = extract_domain(&item.web_url);
                let tags_flat = item.tags.join(" ");
                stmt.bind_text(1, &item.file_path)?;
                stmt.bind_text(2, item.media_type.as_str())?;
                stmt.bind_text(3, &item.capture_date)?;
                stmt.bind_text(4, &item.app_name)?;
                stmt.bind_text(5, &item.window_title)?;
                stmt.bind_text(6, &item.web_url)?;
                stmt.bind_text(7, &domain)?;
                stmt.bind_text(8, &item.callout_text)?;
                stmt.bind_text(9, &item.ocr_text)?;
                stmt.bind_int64(10, item.width as i64)?;
                stmt.bind_int64(11, item.height as i64)?;
                stmt.bind_text(12, &tags_flat)?;
                stmt.step()?;
                stmt.reset()?;
            }
            Ok(())
        })();

        match res {
            Ok(()) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Indexes a `.snagx` file directly from disk into the library database.
    pub fn index_snagx_file(&mut self, path: impl AsRef<Path>) -> Result<i64> {
        let p = path.as_ref();
        let archive = SnagxArchive::from_file(p)?;
        let (w, h) = archive
            .pages
            .first()
            .map(|pg| {
                (
                    pg.page.capture_canvas_width.unwrap_or(0.0).round() as u32,
                    pg.page.capture_canvas_height.unwrap_or(0.0).round() as u32,
                )
            })
            .unwrap_or((0, 0));

        let item = LibraryItem {
            id: 0,
            file_path: p.to_string_lossy().into_owned(),
            media_type: MediaType::Image,
            capture_date: archive.metadata.capture_date.unwrap_or_default(),
            app_name: archive.metadata.app_name.unwrap_or_default(),
            window_title: archive.metadata.window_name.unwrap_or_default(),
            web_url: archive.metadata.web_url.unwrap_or_default(),
            callout_text: archive.metadata.callout_text.unwrap_or_default(),
            ocr_text: archive.metadata.ocr_text.unwrap_or_default(),
            width: w,
            height: h,
            tags: archive.metadata.tags.unwrap_or_default(),
        };
        self.upsert_item(&item)
    }

    pub fn set_item_tags(&mut self, item_id: i64, tags: &[String]) -> Result<()> {
        self.conn.execute_batch("BEGIN IMMEDIATE;")?;
        let res = (|| -> Result<()> {
            {
                let mut del = self
                    .conn
                    .prepare("DELETE FROM item_tags WHERE item_id = ?1")?;
                del.bind_int64(1, item_id)?;
                del.step()?;
            }
            for tag in tags {
                let clean = tag.trim();
                if clean.is_empty() {
                    continue;
                }
                {
                    let mut ins = self
                        .conn
                        .prepare("INSERT OR IGNORE INTO tags(name) VALUES (?1)")?;
                    ins.bind_text(1, clean)?;
                    ins.step()?;
                }
                let tag_id = {
                    let mut sel = self
                        .conn
                        .prepare("SELECT id FROM tags WHERE name = ?1")?;
                    sel.bind_text(1, clean)?;
                    if sel.step()? == SQLITE_ROW {
                        sel.col_i64(0)
                    } else {
                        continue;
                    }
                };
                {
                    let mut link = self.conn.prepare(
                        "INSERT OR IGNORE INTO item_tags(item_id, tag_id) VALUES (?1, ?2)",
                    )?;
                    link.bind_int64(1, item_id)?;
                    link.bind_int64(2, tag_id)?;
                    link.step()?;
                }
            }
            let flat = tags.join(" ");
            {
                let mut upd = self
                    .conn
                    .prepare("UPDATE items SET tags_flat = ?1 WHERE id = ?2")?;
                upd.bind_text(1, &flat)?;
                upd.bind_int64(2, item_id)?;
                upd.step()?;
            }
            Ok(())
        })();

        match res {
            Ok(()) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    pub fn remove_by_path(&mut self, file_path: &str) -> Result<usize> {
        let mut stmt = self
            .conn
            .prepare("DELETE FROM items WHERE file_path = ?1")?;
        stmt.bind_text(1, file_path)?;
        stmt.step()?;
        Ok(self.conn.changes())
    }

    /// Full-text search (`FTS5` with `unicode61 remove_diacritics 2` supporting Vietnamese + English!).
    pub fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<LibraryItem>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT i.id, i.file_path, i.media_type, i.capture_date, i.app_name,
                   i.window_title, i.web_url, i.callout_text, i.ocr_text, i.width, i.height, i.tags_flat
            FROM items_fts f
            JOIN items i ON i.id = f.rowid
            WHERE items_fts MATCH ?1
            ORDER BY bm25(items_fts), i.capture_date DESC
            LIMIT ?2
            "#,
        )?;
        stmt.bind_text(1, query)?;
        stmt.bind_int64(2, limit as i64)?;
        collect_items(&mut stmt)
    }

    pub fn list_recent(&self, limit: usize) -> Result<Vec<LibraryItem>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, file_path, media_type, capture_date, app_name,
                   window_title, web_url, callout_text, ocr_text, width, height, tags_flat
            FROM items
            ORDER BY capture_date DESC, id DESC
            LIMIT ?1
            "#,
        )?;
        stmt.bind_int64(1, limit as i64)?;
        collect_items(&mut stmt)
    }

    pub fn filter_by_media_type(
        &self,
        media_type: MediaType,
        limit: usize,
    ) -> Result<Vec<LibraryItem>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, file_path, media_type, capture_date, app_name,
                   window_title, web_url, callout_text, ocr_text, width, height, tags_flat
            FROM items
            WHERE media_type = ?1
            ORDER BY capture_date DESC
            LIMIT ?2
            "#,
        )?;
        stmt.bind_text(1, media_type.as_str())?;
        stmt.bind_int64(2, limit as i64)?;
        collect_items(&mut stmt)
    }

    pub fn facets_by_app(&self) -> Result<Vec<FacetCount>> {
        self.query_facets(
            "SELECT app_name, COUNT(*) FROM items WHERE app_name != '' GROUP BY app_name ORDER BY COUNT(*) DESC",
        )
    }

    pub fn facets_by_domain(&self) -> Result<Vec<FacetCount>> {
        self.query_facets(
            "SELECT web_domain, COUNT(*) FROM items WHERE web_domain != '' GROUP BY web_domain ORDER BY COUNT(*) DESC",
        )
    }

    pub fn facets_by_year_month(&self) -> Result<Vec<FacetCount>> {
        self.query_facets(
            "SELECT substr(capture_date, 1, 7) AS ym, COUNT(*) FROM items WHERE length(capture_date) >= 7 GROUP BY ym ORDER BY ym DESC",
        )
    }

    pub fn facets_by_tag(&self) -> Result<Vec<FacetCount>> {
        self.query_facets(
            "SELECT t.name, COUNT(it.item_id) FROM tags t JOIN item_tags it ON it.tag_id = t.id GROUP BY t.id ORDER BY COUNT(it.item_id) DESC",
        )
    }

    fn query_facets(&self, sql: &str) -> Result<Vec<FacetCount>> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut out = Vec::new();
        while stmt.step()? == SQLITE_ROW {
            out.push(FacetCount {
                label: stmt.col_text(0),
                count: stmt.col_i64(1) as usize,
            });
        }
        Ok(out)
    }
}

fn collect_items(stmt: &mut SqliteStmt<'_>) -> Result<Vec<LibraryItem>> {
    let mut out = Vec::new();
    while stmt.step()? == SQLITE_ROW {
        let tags_flat = stmt.col_text(11);
        let tags = tags_flat
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        out.push(LibraryItem {
            id: stmt.col_i64(0),
            file_path: stmt.col_text(1),
            media_type: MediaType::from_db_str(&stmt.col_text(2)),
            capture_date: stmt.col_text(3),
            app_name: stmt.col_text(4),
            window_title: stmt.col_text(5),
            web_url: stmt.col_text(6),
            callout_text: stmt.col_text(7),
            ocr_text: stmt.col_text(8),
            width: stmt.col_i64(9) as u32,
            height: stmt.col_i64(10) as u32,
            tags,
        });
    }
    Ok(out)
}

fn extract_domain(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let without_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    without_scheme
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Filesystem watcher using `notify` monitoring a library directory for `.snagx`/`.png`/`.mp4`/`.gif` changes.
pub struct LibraryWatcher {
    _watcher: RecommendedWatcher,
    rx: Receiver<PathBuf>,
}

impl LibraryWatcher {
    pub fn watch_dir(dir: impl AsRef<Path>) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(ev) = res {
                for path in ev.paths {
                    let _ = tx.send(path);
                }
            }
        })?;
        watcher.watch(dir.as_ref(), RecursiveMode::Recursive)?;
        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    pub fn poll_changed_paths(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        while let Ok(p) = self.rx.try_recv() {
            out.push(p);
        }
        out
    }
}
