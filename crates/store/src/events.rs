//! First-class curve-event write/read path for the three new physiologic
//! tables: `meal_event`, `dose_event`, and `basal_schedule_dose`. Each meal or
//! dose crosses the wire as a self-describing curve keyed by the phone's
//! `client_id`; a basal schedule is a daily-repeating template the TUI tiles.
//! The server is a verbatim mirror — it never computes or re-stamps.
//!
//! Idempotency (all on the single phone clock, so no cross-clock hazard):
//!   * meal / dose / basal-slot → upsert-on-newer, i.e.
//!     `ON CONFLICT(client_id) DO UPDATE … WHERE excluded.updated_at > <row>.updated_at`
//!     — a byte-identical redelivery has equal `updated_at` → guard fails → no-op;
//!     a reordered stale redelivery (older `updated_at`) is rejected; a genuine
//!     phone-side edit (newer `updated_at`) replaces in place.
//!
//! The phone `updated_at` is stored **verbatim** (never re-stamped); `received_at`
//! is the server's internal arrival stamp and never crosses the wire. After a
//! guarded upsert that may be a no-op, each writer returns the *canonical* row via
//! a `SELECT … WHERE client_id = ?` (never `last_insert_rowid()`, which is 0 on a
//! no-op) so the api layer can fan it out with `broadcast_except(origin)`.
//!
//! The prediction / stats-block / note / alert writers and readers live in their
//! §5-designated files (`writes.rs`, `reads.rs`, `stats.rs`) and are intentionally
//! NOT duplicated here.

use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};

use t1dm_core::{BasalSchedule, BasalSlot, DoseEvent, DoseKind, MealEvent};

use crate::error::Result;
use crate::{now_ms, Store};

// ----- shared helpers -----------------------------------------------------

/// Parse a stored `custom_curve` JSON text into a per-bucket `f64` vector.
/// A malformed blob degrades to `None` rather than failing the whole read.
fn parse_curve(text: Option<String>) -> Option<Vec<f64>> {
    text.and_then(|s| serde_json::from_str(&s).ok())
}

/// The SQLite text stored for a [`DoseKind`] (matches the `CHECK` constraint).
fn kind_str(kind: DoseKind) -> &'static str {
    match kind {
        DoseKind::Bolus => "bolus",
        DoseKind::Basal => "basal",
    }
}

/// Map stored dose-kind text back to a [`DoseKind`]. Anything unexpected (the
/// `CHECK` constraint forbids it) falls back to `Bolus`.
fn parse_kind(text: &str) -> DoseKind {
    match text {
        "basal" => DoseKind::Basal,
        _ => DoseKind::Bolus,
    }
}

// ----- meal_event ---------------------------------------------------------

const MEAL_COLS: &str =
    "client_id, ts, tz_offset, updated_at, grams, duration_min, gi, k, theta, custom_curve, note";

/// Upsert-on-newer: replace the row only when the incoming phone `updated_at`
/// is strictly newer, so a redelivery is a no-op and a stale reorder is rejected.
const MEAL_UPSERT: &str = r#"
INSERT INTO meal_event
    (client_id, ts, tz_offset, updated_at, grams, duration_min, gi, k, theta, custom_curve, note, received_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
ON CONFLICT(client_id) DO UPDATE SET
    ts           = excluded.ts,
    tz_offset    = excluded.tz_offset,
    updated_at   = excluded.updated_at,
    grams        = excluded.grams,
    duration_min = excluded.duration_min,
    gi           = excluded.gi,
    k            = excluded.k,
    theta        = excluded.theta,
    custom_curve = excluded.custom_curve,
    note         = excluded.note,
    received_at  = excluded.received_at
WHERE excluded.updated_at > meal_event.updated_at
"#;

fn map_meal(row: &Row<'_>) -> rusqlite::Result<MealEvent> {
    Ok(MealEvent {
        client_id: row.get(0)?,
        ts: row.get(1)?,
        tz_offset: row.get(2)?,
        updated_at: row.get(3)?,
        grams: row.get(4)?,
        duration_min: row.get(5)?,
        gi: row.get(6)?,
        k: row.get(7)?,
        theta: row.get(8)?,
        custom_curve: parse_curve(row.get(9)?),
        note: row.get(10)?,
    })
}

fn select_meal(conn: &Connection, client_id: &str) -> Result<Option<MealEvent>> {
    let sql = format!("SELECT {MEAL_COLS} FROM meal_event WHERE client_id = ?1");
    Ok(conn.query_row(&sql, params![client_id], map_meal).optional()?)
}

// ----- dose_event ---------------------------------------------------------

const DOSE_COLS: &str = "client_id, ts, tz_offset, updated_at, kind, units, duration_min, \
     k, theta, ka_per_hour, ke_per_hour, custom_curve, note";

const DOSE_UPSERT: &str = r#"
INSERT INTO dose_event
    (client_id, ts, tz_offset, updated_at, kind, units, duration_min, k, theta, ka_per_hour, ke_per_hour, custom_curve, note, received_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
ON CONFLICT(client_id) DO UPDATE SET
    ts           = excluded.ts,
    tz_offset    = excluded.tz_offset,
    updated_at   = excluded.updated_at,
    kind         = excluded.kind,
    units        = excluded.units,
    duration_min = excluded.duration_min,
    k            = excluded.k,
    theta        = excluded.theta,
    ka_per_hour  = excluded.ka_per_hour,
    ke_per_hour  = excluded.ke_per_hour,
    custom_curve = excluded.custom_curve,
    note         = excluded.note,
    received_at  = excluded.received_at
WHERE excluded.updated_at > dose_event.updated_at
"#;

fn map_dose(row: &Row<'_>) -> rusqlite::Result<DoseEvent> {
    let kind_txt: String = row.get(4)?;
    Ok(DoseEvent {
        client_id: row.get(0)?,
        ts: row.get(1)?,
        tz_offset: row.get(2)?,
        updated_at: row.get(3)?,
        kind: parse_kind(&kind_txt),
        units: row.get(5)?,
        duration_min: row.get(6)?,
        k: row.get(7)?,
        theta: row.get(8)?,
        ka_per_hour: row.get(9)?,
        ke_per_hour: row.get(10)?,
        custom_curve: parse_curve(row.get(11)?),
        note: row.get(12)?,
    })
}

fn select_dose(conn: &Connection, client_id: &str) -> Result<Option<DoseEvent>> {
    let sql = format!("SELECT {DOSE_COLS} FROM dose_event WHERE client_id = ?1");
    Ok(conn.query_row(&sql, params![client_id], map_dose).optional()?)
}

// ----- basal_schedule_dose ------------------------------------------------

const BASAL_COLS: &str = "client_id, schedule_id, label, time_of_day_min, dose_u, duration_min, \
     ka_per_hour, ke_per_hour, tz_offset, updated_at, active";

const BASAL_UPSERT: &str = r#"
INSERT INTO basal_schedule_dose
    (client_id, schedule_id, label, time_of_day_min, dose_u, duration_min, ka_per_hour, ke_per_hour, tz_offset, active, updated_at, received_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
ON CONFLICT(client_id) DO UPDATE SET
    schedule_id     = excluded.schedule_id,
    label           = excluded.label,
    time_of_day_min = excluded.time_of_day_min,
    dose_u          = excluded.dose_u,
    duration_min    = excluded.duration_min,
    ka_per_hour     = excluded.ka_per_hour,
    ke_per_hour     = excluded.ke_per_hour,
    tz_offset       = excluded.tz_offset,
    active          = excluded.active,
    updated_at      = excluded.updated_at,
    received_at     = excluded.received_at
WHERE excluded.updated_at > basal_schedule_dose.updated_at
"#;

/// Map one basal-slot row into `(schedule_id, active, slot)`.
fn map_basal_row(row: &Row<'_>) -> rusqlite::Result<(String, bool, BasalSlot)> {
    let schedule_id: String = row.get(1)?;
    let active: i64 = row.get(10)?;
    let slot = BasalSlot {
        client_id: row.get(0)?,
        label: row.get(2)?,
        time_of_day_min: row.get(3)?,
        dose_u: row.get(4)?,
        duration_min: row.get(5)?,
        ka_per_hour: row.get(6)?,
        ke_per_hour: row.get(7)?,
        tz_offset: row.get(8)?,
        updated_at: row.get(9)?,
    };
    Ok((schedule_id, active != 0, slot))
}

/// Assemble the rows of one `schedule_id` into a [`BasalSchedule`]. `active` is
/// the OR of the rows' active flags; `slots` are ordered by `time_of_day_min`.
fn select_schedule_by_id(conn: &Connection, schedule_id: &str) -> Result<Option<BasalSchedule>> {
    let sql = format!(
        "SELECT {BASAL_COLS} FROM basal_schedule_dose
         WHERE schedule_id = ?1 ORDER BY time_of_day_min ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![schedule_id], map_basal_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        return Ok(None);
    }
    let active = rows.iter().any(|(_, a, _)| *a);
    let slots = rows.into_iter().map(|(_, _, s)| s).collect();
    Ok(Some(BasalSchedule {
        schedule_id: schedule_id.to_string(),
        active,
        slots,
    }))
}

// ==========================================================================
// impl Store — writers + readers
// ==========================================================================

impl Store {
    // ----- meals ----------------------------------------------------------

    /// Idempotent batch upsert of meal curves (key `client_id`, upsert-on-newer).
    /// Returns the canonical row for each input, in request order.
    pub fn put_meals(&self, meals: &[MealEvent]) -> Result<Vec<MealEvent>> {
        let now = now_ms();
        self.with_writer(|conn| {
            let tx = conn.unchecked_transaction()?;
            let mut out = Vec::with_capacity(meals.len());
            for meal in meals {
                let curve_json: Option<String> = meal
                    .custom_curve
                    .as_ref()
                    .map(|c| serde_json::to_string(c))
                    .transpose()?;
                tx.execute(
                    MEAL_UPSERT,
                    params![
                        meal.client_id,
                        meal.ts,
                        meal.tz_offset,
                        meal.updated_at,
                        meal.grams,
                        meal.duration_min,
                        meal.gi,
                        meal.k,
                        meal.theta,
                        curve_json,
                        meal.note,
                        now,
                    ],
                )?;
                if let Some(m) = select_meal(&tx, &meal.client_id)? {
                    out.push(m);
                }
            }
            tx.commit()?;
            Ok(out)
        })
    }

    /// Meals with `ts` in `[from, to]`, ascending.
    pub fn get_meals(&self, from: Option<i64>, to: Option<i64>) -> Result<Vec<MealEvent>> {
        let lo = from.unwrap_or(i64::MIN);
        let hi = to.unwrap_or(i64::MAX);
        self.with_reader(|conn| {
            let sql = format!(
                "SELECT {MEAL_COLS} FROM meal_event WHERE ts >= ?1 AND ts <= ?2 ORDER BY ts ASC"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![lo, hi], map_meal)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// One meal by its phone `client_id`.
    pub fn get_meal(&self, client_id: &str) -> Result<Option<MealEvent>> {
        self.with_reader(|conn| select_meal(conn, client_id))
    }

    // ----- doses ----------------------------------------------------------

    /// Idempotent batch upsert of dose curves (key `client_id`, upsert-on-newer).
    /// Returns the canonical row for each input, in request order.
    pub fn put_doses(&self, doses: &[DoseEvent]) -> Result<Vec<DoseEvent>> {
        let now = now_ms();
        self.with_writer(|conn| {
            let tx = conn.unchecked_transaction()?;
            let mut out = Vec::with_capacity(doses.len());
            for dose in doses {
                let curve_json: Option<String> = dose
                    .custom_curve
                    .as_ref()
                    .map(|c| serde_json::to_string(c))
                    .transpose()?;
                tx.execute(
                    DOSE_UPSERT,
                    params![
                        dose.client_id,
                        dose.ts,
                        dose.tz_offset,
                        dose.updated_at,
                        kind_str(dose.kind),
                        dose.units,
                        dose.duration_min,
                        dose.k,
                        dose.theta,
                        dose.ka_per_hour,
                        dose.ke_per_hour,
                        curve_json,
                        dose.note,
                        now,
                    ],
                )?;
                if let Some(d) = select_dose(&tx, &dose.client_id)? {
                    out.push(d);
                }
            }
            tx.commit()?;
            Ok(out)
        })
    }

    /// Doses with `ts` in `[from, to]`, ascending.
    pub fn get_doses(&self, from: Option<i64>, to: Option<i64>) -> Result<Vec<DoseEvent>> {
        let lo = from.unwrap_or(i64::MIN);
        let hi = to.unwrap_or(i64::MAX);
        self.with_reader(|conn| {
            let sql = format!(
                "SELECT {DOSE_COLS} FROM dose_event WHERE ts >= ?1 AND ts <= ?2 ORDER BY ts ASC"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![lo, hi], map_dose)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// One dose by its phone `client_id`.
    pub fn get_dose(&self, client_id: &str) -> Result<Option<DoseEvent>> {
        self.with_reader(|conn| select_dose(conn, client_id))
    }

    // ----- basal schedule -------------------------------------------------

    /// Full-replace the active basal schedule with `sched`. Each slot is an
    /// upsert-on-newer by `client_id`; when `sched.active`, this schedule becomes
    /// the SOLE active one — every slot of any other schedule is deactivated and
    /// any slot of this schedule absent from the incoming set is dropped (a true
    /// replace). Returns the canonical stored schedule (by `schedule_id`).
    pub fn put_basal_schedule(&self, sched: &BasalSchedule) -> Result<BasalSchedule> {
        let now = now_ms();
        let active_flag: i64 = if sched.active { 1 } else { 0 };
        self.with_writer(|conn| {
            let tx = conn.unchecked_transaction()?;
            for slot in &sched.slots {
                tx.execute(
                    BASAL_UPSERT,
                    params![
                        slot.client_id,
                        sched.schedule_id,
                        slot.label,
                        slot.time_of_day_min,
                        slot.dose_u,
                        slot.duration_min,
                        slot.ka_per_hour,
                        slot.ke_per_hour,
                        slot.tz_offset,
                        active_flag,
                        slot.updated_at,
                        now,
                    ],
                )?;
            }

            if sched.active {
                // Retire any previously-active schedule; only `sched` stays live.
                tx.execute(
                    "UPDATE basal_schedule_dose SET active = 0
                     WHERE schedule_id != ?1 AND active != 0",
                    params![sched.schedule_id],
                )?;
                // Drop slots of this schedule not present in the incoming set.
                if sched.slots.is_empty() {
                    tx.execute(
                        "DELETE FROM basal_schedule_dose WHERE schedule_id = ?1",
                        params![sched.schedule_id],
                    )?;
                } else {
                    let placeholders = std::iter::repeat("?")
                        .take(sched.slots.len())
                        .collect::<Vec<_>>()
                        .join(",");
                    let sql = format!(
                        "DELETE FROM basal_schedule_dose
                         WHERE schedule_id = ? AND client_id NOT IN ({placeholders})"
                    );
                    let mut binds: Vec<&str> = Vec::with_capacity(sched.slots.len() + 1);
                    binds.push(sched.schedule_id.as_str());
                    for slot in &sched.slots {
                        binds.push(slot.client_id.as_str());
                    }
                    tx.execute(&sql, params_from_iter(binds))?;
                }
            }

            let canonical = select_schedule_by_id(&tx, &sched.schedule_id)?;
            tx.commit()?;
            Ok(canonical.unwrap_or_else(|| BasalSchedule {
                schedule_id: sched.schedule_id.clone(),
                active: sched.active,
                slots: Vec::new(),
            }))
        })
    }

    /// The live (active) basal schedule, if one is set.
    pub fn get_basal_schedule(&self) -> Result<Option<BasalSchedule>> {
        self.with_reader(|conn| {
            let sql = format!(
                "SELECT {BASAL_COLS} FROM basal_schedule_dose
                 WHERE active = 1 ORDER BY time_of_day_min ASC"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map([], map_basal_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.is_empty() {
                return Ok(None);
            }
            let schedule_id = rows[0].0.clone();
            let slots = rows.into_iter().map(|(_, _, s)| s).collect();
            Ok(Some(BasalSchedule {
                schedule_id,
                active: true,
                slots,
            }))
        })
    }
}
