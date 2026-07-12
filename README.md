# T1DMSERVER

A single-user diabetes telemetry appliance. One binary is at once an
HTTP/WebSocket server and a live terminal dashboard, backed by an in-process
SQLite store. It ingests continuous physiologic samples, streams them to
connected clients in real time, serves forecasting models, and renders a
themed [ratatui](https://ratatui.rs) TUI over SSH — designed to run
unattended on a Raspberry Pi Zero 2 W and be watched from a tmux session.

The appliance is deliberately scoped to one person's data. There is exactly
one read/write client (the device that owns the data) and any number of
read-only viewers. Authentication is by opaque bearer token, provisioned from
the TUI and handed to a phone by scanning a QR code.

> [!CAUTION]
> **Research and educational use only.** This project is a personal
> telemetry server and dashboard for Type 1 Diabetes data — not a medical
> device, and not clinically validated. The forecasts it serves and the
> statistics it computes are informational only and **must not** be used to
> make medical, diagnostic, or treatment decisions, to calculate or adjust
> insulin doses, or to guide diabetes management in any way. For medical
> advice, consult a qualified healthcare professional. The software is
> provided "as is", without warranty of any kind, and the authors accept no
> liability for any use.

## Features

- **Wide 5-minute grid.** Blood glucose, carbohydrates, bolus, basal, heart
  rate, steps, sleep, exercise, and mood all share one SQLite table on a fixed
  five-minute epoch grid; gaps are explicit `NULL`s. Physiologic storage units
  are fixed (mg/dL, grams, units, units/hour); blood-glucose display toggles
  between mg/dL and mmol/L without touching stored values.
- **Atomic ingest.** A single endpoint accepts an atomic five-minute bundle
  (all series plus an optional forecast and notes); per-series batch upsert and
  backfill are available for bulk imports.
- **Quantile forecasts.** Predictions carry a median line, a seven-level
  quantile fan (`0.05, 0.1, 0.25, 0.5, 0.75, 0.9, 0.95`), and a twelve-bin
  circadian time-of-day distribution with a confidence scalar.
- **Notes, meal photos, and alerts.** Free-text notes and meal photographs are
  pinned to the grid; application alerts fan out over the live stream to every
  connected client except the one that raised them.
- **Rolling statistics.** Time-in-range, time-below and time-above, mean BG,
  GMI, CV, SD, hypo/hyper event counts and durations, mean daily carbohydrates,
  total daily insulin, bolus:basal ratio, mean heart rate, and BG–HR
  correlation, recomputed on ingest across 24-hour, 7-day, and 30-day windows.
- **Real-time stream.** A server-to-client WebSocket pushes new samples,
  predictions, notes, photos, and alerts as tagged JSON.
- **Model registry.** The models directory is scanned at startup and watched
  for changes; each artifact's opaque metadata and its bytes (with a SHA-256
  header) are served over HTTP.
- **Themed terminal UI.** Three complete themes — Tron Legacy, Umbrella Corp,
  and Hello Kitty — each with its own palette, animation vocabulary, boot
  sequence, and glyph set, hot-swappable at runtime. Rendering is
  demand-driven: the UI idles near zero cost and wakes on input, a data event,
  or a live animation. Layout reflows responsively from a wide multi-panel view
  down to a single-column mobile layout for Termux.

## Architecture

A single process hosts everything. A four-worker Tokio runtime serves the axum
HTTP/WebSocket router and watches the models directory; the ratatui TUI owns
the main thread. The SQLite database (WAL mode) is written through one
serialized writer behind a mutex — SQLite admits a single writer — while reads
are served concurrently from an r2d2 connection pool on blocking tasks.
Statistics are computed across cores with rayon. Ingested events reach the TUI
over an in-process broadcast channel, so the dashboard reflects new data the
instant it lands, without polling.

The workspace is four crates plus a root binary:

| Crate | Responsibility |
| --- | --- |
| `crates/core` | Domain types, units, config structs, the cross-crate contract |
| `crates/store` | SQLite schema and migrations, the serialized writer and read pool, all CRUD, tokens and sessions, the model cache, statistics, teardown, and synthetic-data generation |
| `crates/api` | The axum router, bearer-auth middleware, REST handlers, and the WebSocket broadcast hub |
| `crates/tui` | The ratatui application: layout tree, panes, widgets, themes, animation engine, and boot sequences |

## Building

The project targets Rust 1.96 (edition 2021).

### Native (x86_64)

```
cargo build --release
```

The binary is written to `target/release/t1dmserver`. This native build is the
canonical compile gate for the workspace.

### Raspberry Pi Zero 2 W (aarch64)

Because the store links SQLite via `rusqlite`'s bundled C sources, an aarch64
C toolchain is required in addition to the Rust target. Two approaches work.

**Using `cross` (recommended).** [`cross`](https://github.com/cross-rs/cross)
runs the build inside a container image that already carries the cross C
toolchain:

```
cross build --target aarch64-unknown-linux-gnu --release
```

or, equivalently, `just build-pi`.

**Using rustup and a cross linker.** Install the target and an
`aarch64-linux-gnu` GCC, then point Cargo at the cross linker:

```
rustup target add aarch64-unknown-linux-gnu
# install the distro package providing aarch64-linux-gnu-gcc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
cargo build --target aarch64-unknown-linux-gnu --release
```

The artifact lands at `target/aarch64-unknown-linux-gnu/release/t1dmserver`.

### Deploying to the Pi

```
scp target/aarch64-unknown-linux-gnu/release/t1dmserver pi@raspberrypi:~/t1dm/
scp config.example.toml pi@raspberrypi:~/t1dm/config.toml
```

## Running

```
t1dmserver [CONFIG_PATH]
```

The configuration path is taken from the first command-line argument, falling
back to `./config.toml`; if the file is absent or unparseable, built-in
defaults are used. A single invocation launches both the HTTP/WebSocket server
and the TUI, which owns the terminal for its lifetime.

### tmux over SSH

The TUI expects truecolor and mouse support. Add the following to
`~/.tmux.conf` on the Pi so tmux advertises 24-bit colour and forwards mouse
events:

```
set -g default-terminal "tmux-256color"
set -ga terminal-overrides ",*:RGB"
set -g mouse on
```

Launch the appliance inside a named tmux session so it survives SSH
disconnects, then re-attach at will:

```
tmux new -s t1dm '~/t1dm/t1dmserver ~/t1dm/config.toml'
# later, from anywhere:
tmux attach -t t1dm
```

## Configuration

Configuration is a TOML file mirroring `config.example.toml`. Every table has
sensible defaults; an absent file yields a fully-defaulted configuration.

| Key | Default | Meaning |
| --- | --- | --- |
| `server.bind` | `"0.0.0.0"` | Interface the HTTP/WebSocket server binds |
| `server.port` | `8443` | Listen port |
| `storage.data_dir` | `"./data"` | Root holding `t1dm.db`, `models/`, `photos/`, `backups/` |
| `qr.advertise_addr` | `"100.64.0.1"` | Address embedded in the login QR payload |
| `qr.advertise_port` | `8443` | Port embedded in the login QR payload |
| `ui.theme` | `"tron"` | Active theme: `tron`, `umbrella`, or `hellokitty` |
| `ui.fps` | `60` | Frame-rate ceiling (a cap, not a target) |
| `ui.show_boot` | `true` | Play the theme boot sequence on launch |
| `ui.bg_unit` | `"mgdl"` | Blood-glucose display unit: `mgdl` or `mmol` |
| `backup.enabled` | `true` | Enable periodic database backups |
| `backup.path` | `"./data/backups"` | Backup destination directory |
| `backup.interval_hours` | `24` | Hours between backups |
| `log.level` | `"info"` | Tracing level: `error`, `warn`, `info`, `debug`, `trace` |

The `qr.advertise_addr` and `qr.advertise_port` are the values a client should
dial, which may differ from the local bind address — for example a Tailscale or
VPN address reachable from a phone.

## Storage layout

Everything lives under `storage.data_dir`:

```
data/
  t1dm.db          # SQLite database (plus -wal and -shm in WAL mode)
  models/          # forecasting model artifacts, scanned and watched
  photos/          # meal photographs, named by content hash
  backups/         # periodic database copies
```

The database grows by roughly a gigabyte per year and is not compacted. The
development teardown drops and recreates every table and clears the photos
directory; the models directory is left untouched.

## Authentication and QR login

Access is governed by opaque bearer tokens: 32 random bytes rendered as hex.
Tokens are stored only as a salted SHA-256 verifier, never in the clear; they
do not expire and are not rotated, only revoked.

- **One read/write token.** At most one live `rw` token exists at any time — a
  database invariant. Minting a new `rw` token revokes and replaces the
  previous one. The `rw` token authorizes every write endpoint.
- **Read-only tokens, one per device.** Each viewing device gets its own `ro`
  token with an optional operator label. Read-only tokens satisfy read
  endpoints only. Revoking a token is that device's logout.

Tokens are minted and revoked exclusively from the TUI's Sessions pane; there
is no HTTP endpoint for token management. REST requests present the secret as
an `Authorization: Bearer <secret>` header; the WebSocket carries it as a
`?token=<secret>` query parameter. The auth middleware resolves the secret to a
token, upserts a session record (client IP and user-agent), and enforces the
token kind. Sessions persist across WebSocket reconnects.

To enroll a device, the Sessions pane renders a login QR encoding a JSON
payload built from the `[qr]` configuration:

```json
{ "type": "t1dm-login", "token": "<secret>", "addr": "100.64.0.1", "port": 8443 }
```

A companion app scans it and thereafter authenticates with the embedded token
against the advertised address and port.

## API

The full REST and WebSocket contract — every endpoint, its authentication
requirement, request and response shapes with examples, the WebSocket event
schemas, pagination, and the error format — is documented in
[`docs/API.md`](docs/API.md).

## License

Released under the [MIT License](LICENSE).
