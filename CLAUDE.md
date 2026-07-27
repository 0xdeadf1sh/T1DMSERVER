# T1DMSERVER — local rules

## Orient before you edit

The suite's shared rules and this project's accumulated working knowledge live
one directory up, in `../T1DMCOMMON`. Read them first; nothing they hold is
restated here, and this file is only what is local and operational.

- `../T1DMCOMMON/CLAUDE.md`, and in particular **What T1DMSERVER is, and is
  not** — the server's remit is deliberately narrow, and the commonest mistake
  made in this repository is handing it a job that belongs to the phone.
- `../T1DMCOMMON/SPEC/invariants.md` — the five-minute grid, `tz_offset`, units
  and sign conventions, the two risk spaces, curve semantics, authority and
  ordering. Normative: where this repository's code disagrees with it, one of the
  two is a defect and neither may be assumed right.
- `../T1DMCOMMON/PROJECTS/T1DMSERVER.md` — the recast, the storage layout, the
  access model, the console, deployment.
- `docs/API.md` — the wire contract itself. Read the endpoint you are changing.

## What this process is allowed to decide

The phone authors every record; this binary stores what it is given and hands it
back unchanged. Three consequences bite in practice:

- **Never re-stamp.** `updated_at` and `tz_offset` arrive from the client and are
  written verbatim. `updated_at` is the ordering key for every upsert, so
  replacing it with server time silently breaks idempotent redelivery.
- **Never recompute.** Statistics and forecasts arrive computed. There is no
  server-side arithmetic on a stored physiologic value, and a handler that
  "corrects" one is a defect however reasonable the correction looks.
- **Reject only to protect storage.** An off-grid timestamp or an unresolvable
  window label may be refused, because the store keys reconstruction on them. A
  physiologically implausible value may not. That judgement is the client's.

The TUI carries curve mathematics and a risk transform to draw its own panes.
Those are display conveniences: they may never write back into stored data, and a
value derived for a widget never becomes a record.

## Gates

There is **no CI here.** The gate is manual, it is two commands, and both are
required:

```sh
cargo test --workspace
cargo build --release      # or: just build
```

`cargo test -p <lib>` does not build the root binary, so a non-exhaustive match
over a new event variant passes the test gate and fails the release build. And a
cargo invocation piped into `tail` returns *tail's* exit status — capture
`PIPESTATUS[0]` or the failure disappears.

`just fmt` before finishing. `just clippy` fails on the author's machine until
the component is installed for the pinned toolchain; do not treat that as a
finding about the code.

## Where things belong

Four crates plus a root binary: `core` (domain types, units, curve maths,
config), `store` (schema, writer, read pool, tokens, sessions, model registry),
`api` (router, auth extractors, handlers, hub), `tui` (layout, panes, widgets,
themes, animation). A type shared by two crates belongs in `core`, not copied
across the seam.

Two structural rules the runtime depends on: every write goes through the single
serialized writer, and every SQLite read happens on a blocking task rather than
the async runtime. The TUI owns the main thread and renders on demand — it wakes
on input, a store event, or a live animation — so anything that polls in a frame
loop is a defect even when it looks harmless.

## Changing the wire contract

`docs/API.md` is the contract `T1DMDROID` implements against, not a description
of it. Any change to a route, a field, a status code, or a fan-out rule is a
shared-contract change: read `../T1DMCOMMON/skills/shared-contract-change` first,
amend `SPEC/` if the concept lives there, bump `CONTRACT_VERSION`, and **report**
the counterpart the app needs rather than reaching into that repository.

A field added to a handler but not to `docs/API.md` has not been added — the app
is written against the document.

## Never commit

`config.toml`, `/data`, and the database are gitignored; keep them that way. The
configuration carries a private network address and the store holds real patient
records, so neither a real `advertise_addr`, a bearer token, a database, nor a
screenshot of live data may reach a commit — this repository is public. Use
documentation-range addresses and obviously synthetic values in examples.

## Keep `T1DMCOMMON` true

If a change here falsifies something written up there — a clamp that moved, a
gate that changed, a responsibility that shifted — update `T1DMCOMMON` in the
same task, deleting what is no longer so rather than annotating it. If it is not
yours to change, say which file and which claim is now wrong. See *Keeping this
repository true* in `../T1DMCOMMON/CLAUDE.md`.
