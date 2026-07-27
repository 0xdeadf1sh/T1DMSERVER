# T1DMSERVER

A single-user Type 1 Diabetes telemetry appliance. One binary is both an
HTTP/WebSocket server and a live terminal dashboard, backed by an in-process
SQLite store — built to run unattended on a Raspberry Pi Zero 2 W and be watched
from a tmux session over SSH.

The companion phone app is authoritative: it authors every record and computes
every forecast and statistic. The server is a verbatim store and a read-only
fan-out — it interprets nothing, recomputes nothing, and never re-stamps a
client's timestamps. One read/write client owns the data; any number of
read-only viewers may watch.

> [!CAUTION]
> **Research and educational use only.** This is a personal telemetry server and
> dashboard for Type 1 Diabetes data — not a medical device, and not clinically
> validated. The forecasts and statistics it stores are informational only and
> **must not** be used to make medical, diagnostic, or treatment decisions, to
> calculate or adjust insulin doses, or to guide diabetes management in any way.
> For medical advice, consult a qualified healthcare professional. The software
> is provided "as is", without warranty of any kind, and the authors accept no
> liability for any use.

## Features

- **Scalar grid.** Blood glucose, heart rate, steps, sleep, exercise, and mood
  share one SQLite table on a fixed five-minute epoch grid; gaps are explicit
  `NULL`s. Blood glucose is stored in mg/dL and displayed in mg/dL, mmol/L, or
  Kovatchev risk space.
- **Curve events.** Meals, doses, basal schedules, and prediction curves are
  first-class records keyed by a phone-minted `client_id` and upserted
  idempotently against the client's own `updated_at`.
- **Real-time fan-out.** A WebSocket pushes every stored record as tagged JSON
  to all connected clients except the one that wrote it.
- **Model registry.** Files dropped into the models directory are scanned,
  hashed, and served over HTTP alongside their opaque metadata; the registry
  mirrors the directory.
- **Themed terminal UI.** Four themes — Tron Legacy, Umbrella Corp, Hello Kitty,
  and Windows XP — each with its own palette, animations, boot sequence, and
  glyph set, hot-swappable at runtime. Rendering is demand-driven, and the
  layout reflows from a wide multi-panel view down to a single-column Termux
  layout.

## Architecture

A single process hosts everything. A four-worker Tokio runtime serves the axum
router and watches the models directory; the ratatui TUI owns the main thread.
SQLite runs in WAL mode, with every write serialized behind one mutex and reads
served from an r2d2 pool. Stored records reach the TUI over an in-process
broadcast channel, so the dashboard updates without polling.

| Crate | Responsibility |
| --- | --- |
| `crates/core` | Domain types, units, curve maths, config, the cross-crate contract |
| `crates/store` | Schema and migrations, the serialized writer and read pool, CRUD, tokens and sessions, the model registry, teardown, synthetic data |
| `crates/api` | The axum router, bearer-auth extractors, REST handlers, the WebSocket hub |
| `crates/tui` | Layout tree, panes, widgets, themes, animation engine, boot sequences |

## Building

Rust 1.96, edition 2021.

```
cargo build --release   # native — the canonical compile gate
just build-pi           # aarch64 for the Pi, via `cross`
```

`cross` supplies the aarch64 C toolchain that `rusqlite`'s bundled SQLite needs.
Without it, add the `aarch64-unknown-linux-gnu` target and an
`aarch64-linux-gnu` GCC, then point `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER`
and `CC_aarch64_unknown_linux_gnu` at that compiler.

## Running

```
t1dmserver [CONFIG_PATH]   # defaults to ./config.toml
```

One invocation launches both the server and the TUI, which owns the terminal for
its lifetime. `Tab` cycles panes, `t` cycles themes, `h` or `?` opens help, `q`
quits.

Over SSH, run it inside tmux so it survives disconnects — the TUI expects
truecolor and mouse support:

```
# ~/.tmux.conf
set -g default-terminal "tmux-256color"
set -ga terminal-overrides ",*:RGB"
set -g mouse on
```

```
tmux new -s t1dm '~/t1dm/t1dmserver ~/t1dm/config.toml'
tmux attach -t t1dm
```

## Configuration

A TOML file, taken from the first command-line argument or `./config.toml`.
Every key has a default, so an absent or unparseable file yields a
fully-defaulted configuration.

| Key | Default | Meaning |
| --- | --- | --- |
| `server.bind` | `"0.0.0.0"` | Interface the server binds |
| `server.port` | `8443` | Listen port |
| `storage.data_dir` | `"./data"` | Root holding `t1dm.db`, `models/`, `photos/`, and `backups/` |
| `qr.advertise_addr` | `"100.64.0.1"` | Address embedded in the login QR |
| `qr.advertise_port` | `8443` | Port embedded in the login QR |
| `ui.theme` | `"tron"` | `tron`, `umbrella`, `hellokitty`, or `winxp` |
| `ui.fps` | `60` | Frame-rate ceiling (a cap, not a target) |
| `ui.show_boot` | `true` | Play the theme boot sequence on launch |
| `ui.bg_unit` | `"mgdl"` | Display unit: `mgdl`, `mmol`, or `kovachev` |
| `log.level` | `"info"` | `error`, `warn`, `info`, `debug`, or `trace` |

The `qr.advertise_*` values are what a client should dial, which may differ from
the bind address — a Tailscale address, for example. Database backups are taken
on demand from the Settings pane into `<data_dir>/backups`.

## Access

Access is governed by opaque bearer tokens: 32 random bytes rendered as hex,
stored only as a salted SHA-256 verifier, never expiring, revocable. At most one
`rw` token exists at a time — minting a new one replaces the previous — and each
viewing device gets its own `ro` token.

Tokens are minted only from the TUI's Sessions pane; there is no HTTP endpoint
for token management. First light therefore requires terminal access: start the
binary, open the Sessions pane, mint the `rw` token, and scan the QR it renders
to enrol the phone.

> [!WARNING]
> Plain HTTP, permissive CORS, and tokens that never expire: this is a private
> appliance for a tailnet or LAN, and should not be exposed to the open
> internet.

## API

The complete REST and WebSocket contract — every endpoint, its authentication
requirement, request and response shapes, event schemas, and the error format —
is documented in [`docs/API.md`](docs/API.md).

## License

Released under the [MIT License](LICENSE).
