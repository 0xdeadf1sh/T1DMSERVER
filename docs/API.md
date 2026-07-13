# T1DMSERVER HTTP & WebSocket API

This is the complete wire contract for the T1DMSERVER appliance — the
specification a companion client (for example the Android app) implements
against.

All routes are prefixed with `/v1` and exchange `application/json` unless noted
otherwise. The server binds `server.bind:server.port` (default `0.0.0.0:8443`).

## Contents

- [Conventions](#conventions)
- [Authentication](#authentication)
- [Errors](#errors)
- [Object schemas](#object-schemas)
  - [Sample row](#sample-row)
  - [Meal event](#meal-event)
  - [Dose event](#dose-event)
  - [Basal schedule](#basal-schedule)
  - [Prediction](#prediction)
  - [Note](#note)
  - [Photo (metadata)](#photo-metadata)
  - [Alert](#alert)
  - [Model](#model)
  - [Stats](#stats)
- [Write endpoints (require `rw`)](#write-endpoints-require-rw)
  - [POST /v1/ingest](#post-v1ingest)
  - [PUT /v1/meals](#put-v1meals)
  - [PUT /v1/doses](#put-v1doses)
  - [PUT /v1/basal-schedule](#put-v1basal-schedule)
  - [PUT /v1/predictions](#put-v1predictions)
  - [PUT /v1/stats](#put-v1stats)
  - [POST /v1/notes](#post-v1notes)
  - [POST /v1/photos](#post-v1photos)
  - [POST /v1/alerts](#post-v1alerts)
- [Read endpoints (`ro` or `rw`)](#read-endpoints-ro-or-rw)
  - [GET /v1/series](#get-v1series)
  - [GET /v1/meals](#get-v1meals)
  - [GET /v1/doses](#get-v1doses)
  - [GET /v1/basal-schedule](#get-v1basal-schedule)
  - [GET /v1/predictions](#get-v1predictions)
  - [GET /v1/predictions/latest](#get-v1predictionslatest)
  - [GET /v1/notes](#get-v1notes)
  - [GET /v1/alerts](#get-v1alerts)
  - [GET /v1/photos](#get-v1photos)
  - [GET /v1/photos/{id}](#get-v1photosid)
  - [GET /v1/models](#get-v1models)
  - [GET /v1/models/{id}/meta](#get-v1modelsidmeta)
  - [GET /v1/models/{id}/download](#get-v1modelsiddownload)
  - [GET /v1/stats](#get-v1stats)
  - [GET /v1/health](#get-v1health)
- [WebSocket](#websocket)
  - [GET /v1/stream?token=&lt;secret&gt;](#get-v1streamtokensecret)

## Conventions

- **Timestamps** are integers, milliseconds since the Unix epoch (UTC).
- **The 5-minute grid.** Samples, meal events, and dose events sit on a fixed
  five-minute grid: `ts % 300000 == 0`. The phone snaps a meal or dose event to
  the nearest grid point before sending it; a sample or event timestamp off the
  grid is rejected. Notes and alerts carry a wall-clock `ts` and are not snapped.
- **`tz_offset`** is the client's UTC offset in minutes at the sample time
  (e.g. `-300` for UTC−5), carried alongside the timestamp for local-time
  rendering.
- **Phone-authored identity.** Every physiologic record is authored on the
  phone. A sample is keyed by its grid `ts`; a meal, dose, basal slot, note, or
  alert carries a phone-minted `client_id`, unique and stable for the record's
  life. The server's own integer `id` is assigned internally and is never
  accepted on a write.
- **Client `updated_at` is verbatim.** The millisecond `updated_at` a write
  carries is the phone's clock; the server stores and returns it exactly as
  sent, never re-stamping it. It is the ordering key for the idempotent upsert:
  a redelivery with an equal or older `updated_at` is a no-op, a newer one
  replaces the record in place.
- **No server timestamps on the wire.** The server's internal receipt and
  creation stamps (`received_at`, `created_at`) for phone-authored records are
  never present in a read response or a stream frame.
- **Storage units are fixed:** blood glucose in mg/dL; heart rate in bpm, steps
  as a count, and mood as a small integer; `sleep` and `exercise` are stored as
  plain scalar magnitudes. The mg/dL ↔ mmol/L toggle is display-only and never
  affects the wire format.
- **Meals and doses are curves, not sample columns.** A meal's carbohydrate
  appearance curve and a dose's insulin action curve travel as self-describing
  [meal](#meal-event) and [dose](#dose-event) events. A resolved `custom_curve`
  is a JSON array of `f64` sampled on the fixed 300000 ms grid with bucket 0 at
  the event `ts`; a parametric meal or dose instead carries its curve parameters
  and leaves `custom_curve` `null`.
- **Null asymmetry.** On a write (phone → server) an absent optional field may
  be omitted, and for a sample an omitted field leaves the stored value
  untouched. On a read (server → phone) a missing value is explicit `null`,
  never omitted.

## Authentication

Access is by opaque bearer token — 32 random bytes rendered as hex. Tokens are
minted and revoked from the TUI only; there is no HTTP endpoint for token
management.

REST requests present the secret in a header:

```
Authorization: Bearer <secret>
```

The WebSocket carries it as a query parameter (browsers cannot set headers on a
WebSocket handshake): `GET /v1/stream?token=<secret>`.

The middleware resolves the secret to a live token, upserts a session (keyed on
token, client IP, and device/user-agent), enforces the token kind, and then
dispatches the handler. Sessions persist across WebSocket reconnects.

| Kind | Grants |
| --- | --- |
| `rw` | Every endpoint. At most one live `rw` token exists at a time. |
| `ro` | Read endpoints only. One per device, with an optional operator label. |

Every `/v1` endpoint requires a valid bearer token — `GET /v1/health` included. There is no
unauthenticated route.

### Login QR payload

The Sessions pane renders a login QR encoding a single JSON object:

```json
{ "type": "t1dm-login", "token": "<secret>", "addr": "100.64.0.1", "port": 8443 }
```

- `type` — the constant tag `t1dm-login`.
- `token` — the bearer secret (the same value used for `Authorization: Bearer` and `?token=`).
- `addr` / `port` — the operator-configured advertised endpoint (`[qr]` in `config.toml`, typically the
  Tailscale address). A client composes its base URL as `http://addr:port` (transport TLS is moot on the
  tailnet). Minting a fresh `rw` token revokes the prior one, so an older QR's `token` stops working.

## Errors

Errors return the mapped HTTP status with a JSON body:

```json
{ "error": "bad request: unknown series \"foo\"" }
```

| Status | Condition |
| --- | --- |
| 400 Bad Request | Malformed body or query — bad field name, unparseable window, missing multipart part |
| 401 Unauthorized | Missing, unknown, or revoked bearer token |
| 403 Forbidden | A valid `ro` token on a write endpoint |
| 404 Not Found | Unknown resource id (photo, model) |
| 500 Internal Server Error | Store or filesystem failure |

## Object schemas

These canonical shapes are referenced throughout. In read responses every field
is present; optional physiologic fields are `null` when absent.

### Sample row

```json
{
  "ts": 1735689600000,
  "tz_offset": 0,
  "bg": 112.0,
  "hr": 68.0,
  "steps": 30.0,
  "sleep": 0.0,
  "exercise": 0.0,
  "mood": 4,
  "updated_at": 1735689605000
}
```

The six scalar series are `bg, hr, steps, sleep, exercise, mood`. All are
floating point except `mood`, which is an integer. `updated_at` is the phone's
millisecond clock when the row was authored, stored verbatim. Carbohydrates,
bolus, and basal are no longer sample columns — they are [meal](#meal-event)
and [dose](#dose-event) events.

### Meal event

```json
{
  "client_id": "018f9c8a-...-e7",
  "ts": 1735689600000,
  "tz_offset": 0,
  "updated_at": 1735689605000,
  "grams": 40.0,
  "duration_min": 180.0,
  "gi": 0.55,
  "k": 2.0,
  "theta": 30.0,
  "custom_curve": null,
  "note": "oatmeal"
}
```

- `client_id` — a phone-minted, stable identity; the idempotency key.
- `ts` — the grid-snapped event time; bucket 0 of the appearance curve.
- `grams` — carbohydrate mass ingested.
- `duration_min` — the appearance curve's active span, in minutes.
- `gi`, `k`, `theta` — gamma appearance-curve parameters of a parametric meal.
- `custom_curve` — a resolved appearance curve as `[f64]` on the 5-minute grid,
  carried by a mixed or builder meal (with `gi`/`k`/`theta` then `null`); `null`
  for a parametric meal.
- `note` — optional free text.

### Dose event

```json
{
  "client_id": "018f9c8a-...-a1",
  "ts": 1735689600000,
  "tz_offset": 0,
  "updated_at": 1735689605000,
  "kind": "bolus",
  "units": 4.0,
  "duration_min": 300.0,
  "k": 2.0,
  "theta": 40.0,
  "ka_per_hour": null,
  "ke_per_hour": null,
  "custom_curve": null,
  "note": null
}
```

- `kind` — `bolus` (a gamma action curve) or `basal` (a Bateman action curve).
- `units` — insulin units delivered.
- `duration_min` — the action curve's active span, in minutes.
- `k`, `theta` — gamma action-curve parameters of a `bolus`.
- `ka_per_hour`, `ke_per_hour` — Bateman absorption and elimination rates of a
  discrete `basal`.
- `custom_curve` — a resolved action curve as `[f64]` on the 5-minute grid, or
  `null` when the dose is parametric.
- `client_id`, `ts`, `note` — as for a [meal event](#meal-event).

### Basal schedule

```json
{
  "schedule_id": "default",
  "active": true,
  "slots": [
    {
      "client_id": "018f9c8a-...-b2",
      "label": "overnight",
      "time_of_day_min": 0,
      "dose_u": 0.8,
      "duration_min": 480.0,
      "ka_per_hour": 0.9,
      "ke_per_hour": 0.5,
      "tz_offset": 0,
      "updated_at": 1735689605000
    }
  ]
}
```

- `schedule_id` — the template's identity.
- `active` — `true` for the live schedule.
- `slots` — the daily-repeating dose slots. Each slot carries its own
  `client_id`, a `label`, `time_of_day_min` (minutes past local midnight), a
  `dose_u`, a `duration_min`, Bateman `ka_per_hour`/`ke_per_hour`, a `tz_offset`,
  and an `updated_at`. The TUI tiles the slots across the day for display.

### Prediction

```json
{
  "made_at": 1735689600000,
  "model_id": "lstm-v3",
  "updated_at": 1735689605000,
  "horizon_steps": 24,
  "line": [112.0, 118.0, 123.0, "…"],
  "fan": [[…], […], […], […], […], […], […]],
  "circadian": {
    "probs": [0.0, 0.0, 0.1, 0.3, "…"],
    "predicted_hour": 7.5,
    "resultant_r": 0.8,
    "n_bins": 12,
    "bin_hours": 2.0
  }
}
```

- `made_at` — the phone's cycle timestamp for the forecast, stored verbatim;
  with `model_id` it forms the idempotency key.
- `line` — the predicted median series, length `horizon_steps`, in mg/dL.
- `fan` — a `7 × horizon_steps` matrix, one row per quantile level in the exact
  ascending order `[0.05, 0.1, 0.25, 0.5, 0.75, 0.9, 0.95]`. Row index 3 (the
  `0.5` level) equals `line`.
- `circadian` — the model's circadian belief, or `null` when the model has no
  circadian head. `probs` is a distribution over `n_bins` time-of-day bins each
  `bin_hours` wide; `predicted_hour` is the predicted hour-of-day and
  `resultant_r` the concentration of that belief (`0..=1`).

### Note

```json
{ "client_id": "018f9c8a-...-c3", "ts": 1735689600000, "tz_offset": 0, "text": "felt low before lunch", "updated_at": 1735689605000 }
```

A note keeps its wall-clock `ts` and is editable; `client_id` is its identity
and `updated_at` orders edits.

### Photo (metadata)

```json
{
  "id": 3,
  "ts": 1735689600000,
  "path": "photos/9f86d0…d3.jpg",
  "sha256": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
  "width": 1024,
  "height": 768,
  "bytes": 184320,
  "created_at": 1735689601000
}
```

`path` is relative to `storage.data_dir`. The binary is fetched via
`GET /v1/photos/{id}`.

### Alert

```json
{
  "client_id": "018f9c8a-...-d4",
  "ts": 1735689600000,
  "kind": "low",
  "payload": { "bg": 58 },
  "origin_token": 2
}
```

`client_id` is the alert's phone-minted identity; an alert is immutable.
`payload` is opaque JSON, echoed verbatim. `origin_token` is the id of the token
that raised the alert (or `null`); the origin token's own sessions are excluded
from the live-stream fan-out.

### Model

The `id` is the artifact's full filename (extension included), so format
variants of one model — `lstm-v3.pt`, `lstm-v3.onnx` — register as distinct
entries. `name` is the filename stem; `ext` is the lowercased extension (empty
when the file has none).

```json
{
  "id": "lstm-v3.pt",
  "name": "lstm-v3",
  "ext": "pt",
  "path": "models/lstm-v3.pt",
  "meta": { "arch": "lstm", "params": 1200000, "trained": "2026-06-01" },
  "sha256": "…",
  "bytes": 4823104,
  "discovered_at": 1735689000000
}
```

`meta` is **opaque JSON**: stored, served, and rendered verbatim, never
interpreted. `path` is relative to `storage.data_dir`.

### Stats

```json
{
  "window": "7d",
  "updated_at": 1735689605000,
  "tir": 0.72,
  "time_below": 0.04,
  "time_above": 0.24,
  "mean_bg": 148.3,
  "gmi": 6.9,
  "cv": 34.1,
  "sd": 50.6,
  "hypo_events": { "count": 2, "duration_ms": 3600000 },
  "hyper_events": { "count": 5, "duration_ms": 14400000 },
  "mean_daily_carbs": 172.0,
  "tdd": 38.5,
  "bolus_basal_ratio": 1.4,
  "mean_hr": 71.2,
  "bg_hr_corr": -0.18,
  "n_samples": 264
}
```

A stats block is computed on the phone and stored by the server verbatim.
`window` is one of `7d`, `30d`, `90d`. `updated_at` is the phone's millisecond
clock when the block was computed. The three time-fraction fields (`tir`,
`time_below`, `time_above`) are fractions in `0..=1` about the 70–180 mg/dL
range. `gmi` and `cv` are percentages; `tdd` is units/day; `bg_hr_corr` is a
Pearson correlation in `-1..=1`; `n_samples` is the number of grid samples that
contributed BG to the window.

---

## Write endpoints (require `rw`)

### POST /v1/ingest

Atomic five-minute bundle of the scalar series. Physiologic fields are
optional; those present overwrite the row at `ts` in place, those absent leave
the existing values untouched. Writing the row broadcasts a `sample` event to
every live-stream session except those of the origin token.

Request:

```json
{
  "ts": 1735689600000,
  "tz_offset": 0,
  "updated_at": 1735689605000,
  "bg": 112.0,
  "hr": 68.0, "steps": 30.0, "sleep": 0.0, "exercise": 0.0, "mood": 4
}
```

`ts`, `tz_offset`, and `updated_at` are required; every physiologic field is
optional. `updated_at` is the phone's clock and is stored verbatim; the row is
upserted on `ts`, and a partial redelivery re-applies when its `updated_at` is
equal or newer, and is ignored when older. Predictions and notes are written
through their own endpoints ([`PUT /v1/predictions`](#put-v1predictions),
[`POST /v1/notes`](#post-v1notes)), not embedded here.

Response `200`:

```json
{ "ok": true, "ts": 1735689600000 }
```

### PUT /v1/meals

Batch upsert of [meal events](#meal-event). The body is a JSON array — a
single-element array for a freshly logged meal, an N-element array when
re-mirroring history.

Request:

```json
[
  {
    "client_id": "018f9c8a-...-e7",
    "ts": 1735689600000,
    "tz_offset": 0,
    "updated_at": 1735689605000,
    "grams": 40.0,
    "duration_min": 180.0,
    "gi": 0.55,
    "k": 2.0,
    "theta": 30.0,
    "note": "oatmeal"
  }
]
```

`client_id`, `ts`, `updated_at`, `grams`, and `duration_min` are required; the
curve parameters, `custom_curve`, and `note` are optional and omitted when
absent. A parametric meal carries `gi`/`k`/`theta`; a mixed or builder meal
carries its resolved appearance curve in `custom_curve` instead. Idempotent by
`client_id`: a redelivery is a no-op, a newer `updated_at` replaces the row in
place. Fans out a `meal` event to every session except the origin.

Response `200` — the stored `client_id`s, in input order:

```json
{ "ok": true, "ids": ["018f9c8a-...-e7"] }
```

### PUT /v1/doses

Batch upsert of [dose events](#dose-event). Same batch convention as
[meals](#put-v1meals).

Request:

```json
[
  {
    "client_id": "018f9c8a-...-a1",
    "ts": 1735689600000,
    "tz_offset": 0,
    "updated_at": 1735689605000,
    "kind": "bolus",
    "units": 4.0,
    "duration_min": 300.0,
    "k": 2.0,
    "theta": 40.0
  }
]
```

`client_id`, `ts`, `updated_at`, `kind`, `units`, and `duration_min` are
required. A `bolus` carries gamma `k`/`theta`; a discrete `basal` carries
Bateman `ka_per_hour`/`ke_per_hour`; either may instead carry a resolved
`custom_curve`. Idempotent by `client_id`: a redelivery is a no-op, a newer
`updated_at` replaces in place. Fans out a `dose` event to every session except
the origin.

Response `200` — the stored `client_id`s, in input order:

```json
{ "ok": true, "ids": ["018f9c8a-...-a1"] }
```

### PUT /v1/basal-schedule

Full-replace the daily-repeating basal template. The body is one
[schedule](#basal-schedule) with its slots; the server replaces the named
schedule's slots wholesale.

Request:

```json
{
  "schedule_id": "default",
  "active": true,
  "slots": [
    {
      "client_id": "018f9c8a-...-b2",
      "label": "overnight",
      "time_of_day_min": 0,
      "dose_u": 0.8,
      "duration_min": 480.0,
      "ka_per_hour": 0.9,
      "ke_per_hour": 0.5,
      "tz_offset": 0,
      "updated_at": 1735689605000
    }
  ]
}
```

Each slot is idempotent by its `client_id`; a newer `updated_at` replaces a slot
in place. Fans out a `basal_schedule` event to every session except the origin.

Response `200` — the stored slot `client_id`s:

```json
{ "ok": true, "ids": ["018f9c8a-...-b2"] }
```

### PUT /v1/predictions

Upsert one or more predictions. The body is a JSON array of
[prediction](#prediction) write objects.

Request:

```json
[
  {
    "made_at": 1735689600000,
    "model_id": "lstm-v3",
    "updated_at": 1735689605000,
    "horizon_steps": 24,
    "line": [112.0],
    "fan": [[…], […], […], […], […], […], […]],
    "circadian": { "probs": [0.0, 0.1], "predicted_hour": 7.5, "resultant_r": 0.8, "n_bins": 12, "bin_hours": 2.0 }
  }
]
```

`made_at`, `model_id`, `updated_at`, `horizon_steps`, `line`, and `fan` are
required; `circadian` is `null` when the model has no circadian head. `made_at`
is the phone's cycle timestamp, stored verbatim. Keyed idempotent on `(made_at,
model_id)`: re-running a cycle overwrites its prediction in place, a
byte-identical redelivery is a no-op. Fans out a `prediction` event to every
session except the origin.

Response `200` — the canonical row ids, in input order:

```json
{ "ok": true, "ids": [42] }
```

### PUT /v1/stats

Push a computed [statistics block](#stats) for one window. The phone computes
the block; the server stores it verbatim and serves it back through
[`GET /v1/stats`](#get-v1stats), never re-deriving it.

Request:

```json
{
  "window": "7d",
  "updated_at": 1735689605000,
  "tir": 0.72,
  "time_below": 0.04,
  "time_above": 0.24,
  "mean_bg": 148.3,
  "gmi": 6.9,
  "cv": 34.1,
  "sd": 50.6,
  "hypo_events": { "count": 2, "duration_ms": 3600000 },
  "hyper_events": { "count": 5, "duration_ms": 14400000 },
  "mean_daily_carbs": 172.0,
  "tdd": 38.5,
  "bolus_basal_ratio": 1.4,
  "mean_hr": 71.2,
  "bg_hr_corr": -0.18,
  "n_samples": 264
}
```

`window` (`7d`|`30d`|`90d`) and `updated_at` are required. Keyed idempotent on
`window`: a newer `updated_at` replaces the stored block in place. Fans out a
`stats` event to every session except the origin.

Response `200`:

```json
{ "ok": true, "window": "7d" }
```

### POST /v1/notes

Request:

```json
{ "client_id": "018f9c8a-...-c3", "ts": 1735689600000, "tz_offset": 0, "text": "note body", "updated_at": 1735689605000 }
```

`client_id` and `updated_at` are required; `tz_offset` defaults to `0` if
omitted. A note keeps its wall-clock `ts` and is editable — keyed by
`client_id`, a newer `updated_at` replaces its text in place. Fans out a `note`
event to every session except the origin.

Response `200`:

```json
{ "ok": true, "id": "018f9c8a-...-c3" }
```

### POST /v1/photos

`multipart/form-data` with two parts: a text field `ts`, and an image file in a
field named `image`, `file`, or `photo`. The file extension determines the
stored format; the binary is written under `<data_dir>/photos/<sha256>.<ext>`.
Broadcasts a `photo` event. Dimensions are `0` here and filled in by the
importer/TUI when the image is decoded.

Response `200`:

```json
{ "ok": true, "id": 3, "sha256": "9f86d081…" }
```

### POST /v1/alerts

Raise an application alert. The caller's token is recorded as the alert origin,
and the hub broadcasts the alert to every connected session **except** those of
the origin token.

Request:

```json
{ "client_id": "018f9c8a-...-d4", "ts": 1735689600000, "kind": "low", "payload": { "bg": 58 } }
```

`client_id` is required; `payload` is optional and opaque (defaults to `null`).
An alert is immutable — a redelivery of the same `client_id` is ignored.

Response `200`:

```json
{ "ok": true, "id": "018f9c8a-...-d4" }
```

---

## Read endpoints (`ro` or `rw`)

### GET /v1/series

Fetch wide sample rows over a time range, paginated forward by timestamp.

Query parameters:

| Param | Type | Meaning |
| --- | --- | --- |
| `fields` | csv | Comma-separated series allowlist (e.g. `bg,hr,steps`); default all. An unknown name is `400`. |
| `from` | int | Inclusive lower `ts` bound. |
| `to` | int | Inclusive upper `ts` bound. |
| `limit` | int | Maximum rows to return; default `10000`. |
| `cursor` | int | Continuation cursor; rows with `ts <= cursor` are excluded. |

Rows are returned in ascending `ts` order and always carry the full wide schema
(every series column present, `null` for gaps) regardless of `fields`.

Response `200`:

```json
{
  "rows": [
    { "ts": 1735689600000, "tz_offset": 0, "bg": 112.0, "hr": 68.0, "steps": null, "sleep": null, "exercise": null, "mood": null, "updated_at": 1735689605000 }
  ],
  "next_cursor": 1735689600000
}
```

`next_cursor` is the `ts` of the last row returned (or `null` when the page is
empty). To page, pass it back as `cursor` until a request returns no rows.

### GET /v1/meals

Fetch [meal events](#meal-event) over a time range. Query: `from`, `to`
(inclusive bounds on the grid `ts`). Newest first.

Response `200`:

```json
{
  "meals": [
    { "client_id": "018f9c8a-...-e7", "ts": 1735689600000, "tz_offset": 0, "updated_at": 1735689605000, "grams": 40.0, "duration_min": 180.0, "gi": 0.55, "k": 2.0, "theta": 30.0, "custom_curve": null, "note": "oatmeal" }
  ]
}
```

Every field is present; an absent optional is explicit `null`. The server's
internal `id` and stamps are not returned.

### GET /v1/doses

Fetch [dose events](#dose-event) over a time range. Query: `from`, `to`
(inclusive bounds on the grid `ts`). Newest first.

Response `200`:

```json
{
  "doses": [
    { "client_id": "018f9c8a-...-a1", "ts": 1735689600000, "tz_offset": 0, "updated_at": 1735689605000, "kind": "bolus", "units": 4.0, "duration_min": 300.0, "k": 2.0, "theta": 40.0, "ka_per_hour": null, "ke_per_hour": null, "custom_curve": null, "note": null }
  ]
}
```

### GET /v1/basal-schedule

The active basal template (the [Basal schedule](#basal-schedule) schema).

Response `200`:

```json
{
  "schedule_id": "default",
  "active": true,
  "slots": [
    { "client_id": "018f9c8a-...-b2", "label": "overnight", "time_of_day_min": 0, "dose_u": 0.8, "duration_min": 480.0, "ka_per_hour": 0.9, "ke_per_hour": 0.5, "tz_offset": 0, "updated_at": 1735689605000 }
  ]
}
```

When no schedule is set, `slots` is empty.

### GET /v1/predictions

Query: `from`, `to` (inclusive bounds on `made_at`). Newest first.

Response `200`:

```json
{ "predictions": [ { "made_at": 1735689600000, "model_id": "lstm-v3", "…": "…" } ] }
```

### GET /v1/predictions/latest

The single most recent prediction, or `null`.

Response `200`:

```json
{ "prediction": { "made_at": 1735689600000, "…": "…" } }
```

### GET /v1/notes

Query: `from`, `to` (inclusive bounds on `ts`). Newest first.

Response `200`:

```json
{ "notes": [ { "client_id": "018f9c8a-...-c3", "ts": 1735689600000, "tz_offset": 0, "text": "…", "updated_at": 1735689605000 } ] }
```

### GET /v1/alerts

Query: `from`, `to`. Newest first.

Response `200`:

```json
{ "alerts": [ { "client_id": "018f9c8a-...-d4", "ts": 1735689600000, "kind": "low", "payload": { "bg": 58 }, "origin_token": 2 } ] }
```

### GET /v1/photos

Photo metadata over a range. Query: `from`, `to`. Newest first. Returns the
[Photo](#photo-metadata) objects, not the binaries.

```json
{ "photos": [ { "id": 3, "ts": 1735689600000, "path": "photos/…jpg", "sha256": "…", "width": 1024, "height": 768, "bytes": 184320, "created_at": 1735689601000 } ] }
```

### GET /v1/photos/{id}

The image binary. Responds with the appropriate `Content-Type`
(`image/jpeg`, `image/png`, or `image/webp`). `404` if the id is unknown.

### GET /v1/models

The discovered model registry. Any file dropped into `models/` (of any
extension) is registered; `.json` meta sidecars and dotfiles are not.

```json
{ "models": [
  { "id": "lstm-v3.pt", "name": "lstm-v3", "ext": "pt", "path": "models/lstm-v3.pt", "meta": { "…": "…" }, "sha256": "…", "bytes": 4823104, "discovered_at": 1735689000000 },
  { "id": "lstm-v3.onnx", "name": "lstm-v3", "ext": "onnx", "path": "models/lstm-v3.onnx", "meta": { "…": "…" }, "sha256": "…", "bytes": 3910016, "discovered_at": 1735689100000 }
] }
```

### GET /v1/models/{id}/meta

The opaque `meta` JSON for one model, returned verbatim (not wrapped). `404` if
the id is unknown.

```json
{ "arch": "lstm", "params": 1200000, "trained": "2026-06-01" }
```

### GET /v1/models/{id}/download

Streams the model artifact as `application/octet-stream`, regardless of its
format. Response headers carry `Content-Length`, `X-SHA256` (the artifact's
content hash) for integrity verification, and `Content-Disposition` with the
artifact's real filename and extension. `404` if the id is unknown.

### GET /v1/stats

Query: `window` = `7d` | `30d` | `90d` (default `7d`). An unrecognized window
is `400`.

Returns the most recent block the phone pushed for that window via
[`PUT /v1/stats`](#put-v1stats), verbatim — the server never computes or
re-derives statistics. When no block has been pushed for the window, an
all-zero block is returned.

Response `200`:

```json
{ "stats": { "window": "7d", "tir": 0.72, "…": "…" } }
```

See the [Stats](#stats) schema for the full field set.

### GET /v1/health

Liveness probe. Requires a valid bearer token, like every other endpoint.

```json
{ "status": "ok", "ws_clients": 3 }
```

`ws_clients` is the current number of connected WebSocket subscribers.

---

## WebSocket

### GET /v1/stream?token=&lt;secret&gt;

A server-to-client push stream. Authentication is the `token` query parameter;
an invalid or revoked token is rejected with `401` before the upgrade. After the
upgrade the server sends events as they occur; inbound frames from the client
are ignored (a Close frame ends the stream). Either `rw` or `ro` tokens may
subscribe. Sessions — and thus the record of a connected viewer — persist across
reconnects.

Each event is a JSON object with a `"type"` discriminant and the event's fields
inlined alongside it. The nine event types — `sample`, `prediction`, `note`,
`photo`, `alert`, `meal`, `dose`, `basal_schedule`, and `stats` — carry,
respectively, the [Sample row](#sample-row), [Prediction](#prediction),
[Note](#note), [Photo](#photo-metadata), [Alert](#alert),
[Meal event](#meal-event), [Dose event](#dose-event),
[Basal schedule](#basal-schedule), and [Stats](#stats) schemas.

```json
{ "type": "sample",         "ts": 1735689600000, "tz_offset": 0, "bg": 112.0, "hr": 68.0, "steps": null, "sleep": null, "exercise": null, "mood": null, "updated_at": 1735689605000 }
```

```json
{ "type": "prediction",     "made_at": 1735689600000, "model_id": "lstm-v3", "updated_at": 1735689605000, "horizon_steps": 24, "line": [112.0], "fan": [[…]], "circadian": { "probs": [0.0, 0.1], "predicted_hour": 7.5, "resultant_r": 0.8, "n_bins": 12, "bin_hours": 2.0 } }
```

```json
{ "type": "note",           "client_id": "018f9c8a-...-c3", "ts": 1735689600000, "tz_offset": 0, "text": "felt low before lunch", "updated_at": 1735689605000 }
```

```json
{ "type": "photo",          "id": 3, "ts": 1735689600000, "path": "photos/…jpg", "sha256": "…", "width": 1024, "height": 768, "bytes": 184320, "created_at": 1735689601000 }
```

```json
{ "type": "alert",          "client_id": "018f9c8a-...-d4", "ts": 1735689600000, "kind": "low", "payload": { "bg": 58 }, "origin_token": 2 }
```

```json
{ "type": "meal",           "client_id": "018f9c8a-...-e7", "ts": 1735689600000, "tz_offset": 0, "updated_at": 1735689605000, "grams": 40.0, "duration_min": 180.0, "gi": 0.55, "k": 2.0, "theta": 30.0, "custom_curve": null, "note": "oatmeal" }
```

```json
{ "type": "dose",           "client_id": "018f9c8a-...-a1", "ts": 1735689600000, "tz_offset": 0, "updated_at": 1735689605000, "kind": "bolus", "units": 4.0, "duration_min": 300.0, "k": 2.0, "theta": 40.0, "ka_per_hour": null, "ke_per_hour": null, "custom_curve": null, "note": null }
```

```json
{ "type": "basal_schedule", "schedule_id": "default", "active": true, "slots": [ { "client_id": "018f9c8a-...-b2", "label": "overnight", "time_of_day_min": 0, "dose_u": 0.8, "duration_min": 480.0, "ka_per_hour": 0.9, "ke_per_hour": 0.5, "tz_offset": 0, "updated_at": 1735689605000 } ] }
```

```json
{ "type": "stats",          "window": "7d", "updated_at": 1735689605000, "tir": 0.72, "…": "…" }
```

**Every write fans out except the origin.** A write is delivered to every
connected session **except** those belonging to the token that authored it, so a
client never receives an echo of its own write. Because the phone is the sole
read-write author, in practice it never receives a live `sample`, `meal`,
`dose`, `basal_schedule`, or `stats` event; it hydrates any history missed while
offline through REST catch-up (`GET /v1/series`, `GET /v1/meals`,
`GET /v1/doses`), not the stream.
