//! Model registry. Scans the models directory for artifact files of any
//! extension, hashing and upserting each. Model `meta` is OPAQUE JSON — stored
//! and served verbatim, never interpreted.

use std::collections::HashSet;
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
    /// rows. A sibling sidecar, if present, supplies the opaque meta: the
    /// client's `<stem>.descriptor.json` spelling is preferred (that is what the
    /// phone writes and consumes, so a descriptor carried over from its models
    /// directory is found), with `<stem>.json` as the fallback. `.json` sidecars
    /// of either spelling and dotfiles (e.g. `.gitkeep`) are skipped as
    /// artifacts. Returns the current model list afterwards.
    ///
    /// The scan MIRRORS the directory: rows whose artifact is no longer on disk
    /// are dropped, so a retired model stops being listed and stops advertising
    /// a `/download` that would 404. Pruning is gated on the directory actually
    /// existing — an absent or not-yet-mounted models dir leaves the registry
    /// untouched rather than emptying it.
    pub fn refresh_models(&self, dir: &Path) -> Result<Vec<Model>> {
        let now = now_ms();
        if dir.exists() {
            let mut present: HashSet<String> = HashSet::new();
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
                // Stored (and served) relative to `data_dir`, as `photo.path`
                // is: the appliance's filesystem layout is not a read-only
                // client's business. `model_path` rejoins the root, so the
                // download handler still opens a real file.
                let path_str = path
                    .strip_prefix(self.data_dir())
                    .map(|rel| rel.to_string_lossy().to_string())
                    .unwrap_or_else(|_| path.to_string_lossy().to_string());
                present.insert(id.clone());

                let descriptor_path = path.with_file_name(format!("{stem}.descriptor.json"));
                let meta_path = if descriptor_path.exists() {
                    descriptor_path
                } else {
                    path.with_extension("json")
                };
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
            self.with_writer(|conn| {
                let mut stmt = conn.prepare("SELECT id FROM model")?;
                let known = stmt
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for id in known.iter().filter(|id| !present.contains(id.as_str())) {
                    conn.execute("DELETE FROM model WHERE id = ?1", params![id])?;
                }
                Ok(())
            })?;
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

    /// Filesystem path of a model artifact, if registered. The registry stores
    /// (and serves) the `models/<file>` path relative to `data_dir`; this
    /// rejoins the root so a caller can open the artifact.
    pub fn model_path(&self, id: &str) -> Result<Option<PathBuf>> {
        let rel: Option<String> = self.with_reader(|conn| {
            Ok(conn
                .query_row("SELECT path FROM model WHERE id = ?1", params![id], |r| {
                    r.get(0)
                })
                .optional()?)
        })?;
        Ok(rel.map(|r| self.data_dir().join(r)))
    }
}
