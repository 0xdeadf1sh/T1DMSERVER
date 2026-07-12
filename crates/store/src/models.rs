//! Model registry. Scans the models directory for artifact files of any
//! extension, hashing and upserting each. Model `meta` is OPAQUE JSON — stored
//! and served verbatim, never interpreted.

use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension};
use serde_json::Value;

use t1dm_core::Model;

use crate::error::Result;
use crate::writes::sha256_hex;
use crate::{now_ms, Store};

impl Store {
    /// Scan `dir` for model artifacts of any extension, (re)hash them, and
    /// upsert their registry rows. The registry `id` is the full filename, so
    /// format variants of one model (`net.pt`, `net.onnx`) coexist as distinct
    /// rows. A sibling `<stem>.json`, if present, supplies the opaque meta.
    /// `.json` sidecars and dotfiles (e.g. `.gitkeep`) are skipped. Returns the
    /// current model list afterwards.
    pub fn refresh_models(&self, dir: &Path) -> Result<Vec<Model>> {
        let now = now_ms();
        if dir.exists() {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Some(fname) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                // Skip dotfiles (`.gitkeep`) and the opaque-meta sidecars.
                if fname.starts_with('.')
                    || path.extension().and_then(|e| e.to_str()) == Some("json")
                {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let data = std::fs::read(&path)?;
                let sha = sha256_hex(&data);
                let bytes = data.len() as i64;
                let id = fname.to_string();
                let name = stem.to_string();
                let path_str = path.to_string_lossy().to_string();

                let meta_path = path.with_extension("json");
                let meta_txt = if meta_path.exists() {
                    std::fs::read_to_string(&meta_path)?
                } else {
                    "null".to_string()
                };
                // Validate it parses, but persist verbatim.
                let meta_txt = match serde_json::from_str::<Value>(&meta_txt) {
                    Ok(_) => meta_txt,
                    Err(_) => "null".to_string(),
                };

                self.with_writer(|conn| {
                    conn.execute(
                        "INSERT INTO model(id, name, path, meta, sha256, bytes, discovered_at)
                         VALUES (?1,?2,?3,?4,?5,?6,?7)
                         ON CONFLICT(id) DO UPDATE SET
                            name = excluded.name,
                            path = excluded.path,
                            meta = excluded.meta,
                            sha256 = excluded.sha256,
                            bytes = excluded.bytes",
                        params![id, name, path_str, meta_txt, sha, bytes, now],
                    )?;
                    Ok(())
                })?;
            }
        }
        self.list_models()
    }

    /// All registered models, by discovery time descending.
    pub fn list_models(&self) -> Result<Vec<Model>> {
        self.with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, path, meta, sha256, bytes, discovered_at
                 FROM model ORDER BY discovered_at DESC",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    let meta_txt: String = r.get(3)?;
                    let path: String = r.get(2)?;
                    let ext = Path::new(&path)
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.to_ascii_lowercase())
                        .unwrap_or_default();
                    Ok(Model {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        ext,
                        path,
                        meta: serde_json::from_str(&meta_txt).unwrap_or(Value::Null),
                        sha256: r.get(4)?,
                        bytes: r.get(5)?,
                        discovered_at: r.get(6)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// The opaque meta JSON for a model, served verbatim.
    pub fn get_model_meta(&self, id: &str) -> Result<Option<Value>> {
        self.with_reader(|conn| {
            let txt: Option<String> = conn
                .query_row("SELECT meta FROM model WHERE id = ?1", params![id], |r| {
                    r.get(0)
                })
                .optional()?;
            Ok(txt.map(|t| serde_json::from_str(&t).unwrap_or(Value::Null)))
        })
    }

    /// Filesystem path of a model artifact, if registered.
    pub fn model_path(&self, id: &str) -> Result<Option<PathBuf>> {
        self.with_reader(|conn| {
            let p: Option<String> = conn
                .query_row("SELECT path FROM model WHERE id = ?1", params![id], |r| {
                    r.get(0)
                })
                .optional()?;
            Ok(p.map(PathBuf::from))
        })
    }
}
