//! Tokens and sessions. Tokens are opaque 32-byte secrets, stored only as
//! `salt$hash` where `hash = sha256(salt || secret)`. Minting an RW token
//! revokes-and-replaces any live RW to uphold the `one_rw` invariant.

use rusqlite::{params, OptionalExtension, Row};

use t1dm_core::{Session, Token, TokenKind};

use crate::error::{Result, StoreError};
use crate::writes::{sha256_hex, to_hex};
use crate::{now_ms, Store};

/// Compute the stored `salt$hash` verifier for a secret.
fn make_verifier(secret: &str) -> String {
    use rand::{rng, RngExt};
    let salt: [u8; 16] = rng().random();
    let hash = hash_with_salt(&salt, secret);
    format!("{}${}", to_hex(&salt), hash)
}

/// `sha256(salt || secret)` as lowercase hex.
fn hash_with_salt(salt: &[u8], secret: &str) -> String {
    let mut buf = Vec::with_capacity(salt.len() + secret.len());
    buf.extend_from_slice(salt);
    buf.extend_from_slice(secret.as_bytes());
    sha256_hex(&buf)
}

/// Constant-time-ish comparison of a candidate secret against a stored
/// `salt$hash` verifier.
fn verify(verifier: &str, secret: &str) -> bool {
    let Some((salt_hex, want)) = verifier.split_once('$') else {
        return false;
    };
    let Some(salt) = decode_hex(salt_hex) else {
        return false;
    };
    let got = hash_with_salt(&salt, secret);
    // Length-equal hex strings; fold to reduce trivial early-exit leakage.
    got.len() == want.len()
        && got
            .bytes()
            .zip(want.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn map_token(row: &Row<'_>) -> rusqlite::Result<Token> {
    let kind_txt: String = row.get(2)?;
    let kind = if kind_txt == "rw" {
        TokenKind::Rw
    } else {
        TokenKind::Ro
    };
    Ok(Token {
        id: row.get(0)?,
        secret_hash: row.get(1)?,
        kind,
        label: row.get(3)?,
        created_at: row.get(4)?,
        revoked_at: row.get(5)?,
    })
}

const TOKEN_COLS: &str = "id, secret_hash, kind, label, created_at, revoked_at";

impl Store {
    /// Mint a fresh token, returning the persisted [`Token`] and the raw
    /// secret (shown to the operator exactly once). Minting RW first revokes
    /// any live RW token.
    pub fn mint_token(&self, kind: TokenKind, label: Option<String>) -> Result<(Token, String)> {
        use rand::{rng, RngExt};
        let secret_bytes: [u8; 32] = rng().random();
        let secret = to_hex(&secret_bytes);
        let verifier = make_verifier(&secret);
        let now = now_ms();

        let token = self.with_writer(|conn| {
            let tx = conn.unchecked_transaction()?;
            if kind == TokenKind::Rw {
                tx.execute(
                    "UPDATE token SET revoked_at = ?1 WHERE kind = 'rw' AND revoked_at IS NULL",
                    params![now],
                )?;
            }
            tx.execute(
                "INSERT INTO token(secret_hash, kind, label, created_at, revoked_at)
                 VALUES (?1, ?2, ?3, ?4, NULL)",
                params![verifier, kind.as_str(), label, now],
            )?;
            let id = tx.last_insert_rowid();
            tx.commit()?;
            Ok(Token {
                id,
                secret_hash: verifier,
                kind,
                label,
                created_at: now,
                revoked_at: None,
            })
        })?;

        Ok((token, secret))
    }

    /// Revoke a token by id (idempotent for already-revoked tokens).
    pub fn revoke_token(&self, id: i64) -> Result<()> {
        let now = now_ms();
        self.with_writer(|conn| {
            let n = conn.execute(
                "UPDATE token SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
                params![now, id],
            )?;
            // Non-existent id is an error; already-revoked is a no-op success.
            let exists: bool = conn
                .query_row("SELECT 1 FROM token WHERE id = ?1", params![id], |_| {
                    Ok(true)
                })
                .optional()?
                .unwrap_or(false);
            if !exists {
                return Err(StoreError::TokenNotFound(id));
            }
            let _ = n;
            Ok(())
        })
    }

    /// Resolve a raw secret to its live [`Token`], or `None` if unknown or
    /// revoked.
    pub fn verify_secret(&self, secret: &str) -> Result<Option<Token>> {
        self.with_reader(|conn| {
            let sql = format!("SELECT {TOKEN_COLS} FROM token WHERE revoked_at IS NULL");
            let mut stmt = conn.prepare(&sql)?;
            let candidates = stmt
                .query_map([], map_token)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for tok in candidates {
                if verify(&tok.secret_hash, secret) {
                    return Ok(Some(tok));
                }
            }
            Ok(None)
        })
    }

    /// List tokens; when `include_revoked` is false, only live ones.
    pub fn list_tokens(&self, include_revoked: bool) -> Result<Vec<Token>> {
        self.with_reader(|conn| {
            let sql = if include_revoked {
                format!("SELECT {TOKEN_COLS} FROM token ORDER BY created_at DESC")
            } else {
                format!(
                    "SELECT {TOKEN_COLS} FROM token WHERE revoked_at IS NULL ORDER BY created_at DESC"
                )
            };
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map([], map_token)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Upsert a session for `(token_id, ip, device)`, bumping `last_seen`.
    /// Sessions persist across WS reconnects.
    pub fn upsert_session(
        &self,
        token_id: i64,
        ip: &str,
        user_agent: &str,
        device: &str,
    ) -> Result<Session> {
        let now = now_ms();
        self.with_writer(|conn| {
            conn.execute(
                "INSERT INTO session(token_id, ip, user_agent, device, first_seen, last_seen)
                 VALUES (?1,?2,?3,?4,?5,?5)
                 ON CONFLICT(token_id, ip, device) DO UPDATE SET
                    last_seen = excluded.last_seen,
                    user_agent = excluded.user_agent",
                params![token_id, ip, user_agent, device, now],
            )?;
            let sess = conn.query_row(
                "SELECT id, token_id, ip, user_agent, device, first_seen, last_seen
                 FROM session WHERE token_id = ?1 AND ip = ?2 AND device = ?3",
                params![token_id, ip, device],
                |r| {
                    Ok(Session {
                        id: r.get(0)?,
                        token_id: r.get(1)?,
                        ip: r.get(2)?,
                        user_agent: r.get(3)?,
                        device: r.get(4)?,
                        first_seen: r.get(5)?,
                        last_seen: r.get(6)?,
                    })
                },
            )?;
            Ok(sess)
        })
    }

    /// All sessions, most-recently-seen first.
    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        self.with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, token_id, ip, user_agent, device, first_seen, last_seen
                 FROM session ORDER BY last_seen DESC",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(Session {
                        id: r.get(0)?,
                        token_id: r.get(1)?,
                        ip: r.get(2)?,
                        user_agent: r.get(3)?,
                        device: r.get(4)?,
                        first_seen: r.get(5)?,
                        last_seen: r.get(6)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }
}
