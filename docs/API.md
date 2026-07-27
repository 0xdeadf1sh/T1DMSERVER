# HTTP & WebSocket API

The wire contract this server implements is specified once, for the whole suite,
in **`T1DMCOMMON/SPEC/http-api.md`**. It is not restated here: two copies of a
contract are two contracts, and the second one is always the one that quietly
stops being true.

- Repository: <https://github.com/0xdeadf1sh/T1DMCOMMON>
- Sibling checkout: `../T1DMCOMMON/SPEC/http-api.md`
- The version this server currently speaks: `../T1DMCOMMON/CONTRACT_VERSION`

## Where it is implemented here

| Concern | Where |
| --- | --- |
| Router, routes, body limit | `crates/api/src/lib.rs` |
| Bearer auth, session upsert, token kinds | `crates/api/src/auth.rs`, `extract.rs` |
| Handlers, per-endpoint validation | `crates/api/src/handlers.rs` |
| Error mapping to the documented statuses | `crates/api/src/error.rs` |
| Fan-out to every session but the origin | `crates/api/src/hub.rs` |
| Stored shapes and their serialization | `crates/core/src/events.rs`, `domain.rs` |

## Changing it

A change to a route, a field, a status code, or a fan-out rule is a
shared-contract change, not a local one. Amend
`T1DMCOMMON/SPEC/http-api.md` first, bump `CONTRACT_VERSION`, then change the
code here — and report the counterpart `T1DMDROID` needs rather than reaching
into that repository. The protocol is in
`T1DMCOMMON/skills/shared-contract-change`.
