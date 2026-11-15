# Protocols

Three protocols meet in this platform. Types are defined once in
`crates/core-protocol`; everything else consumes them.

## 1. Machine connection protocol

Transport: WebSocket, text frames, one JSON `ConnectionEnvelope` per frame.

```json
{
  "version": 1,
  "stream_id": "default",
  "message_id": "msg_...",
  "type": "request",
  "method": "agent.sessions.create",
  "payload": { }
}
```

`type` is one of `request`, `response`, `event`, `error`. Responses echo the
request's `message_id`. Errors carry an `ApiError` payload.

### Lifecycle

```
daemon → agent admin      connection.hello        HelloPayload
agent admin → daemon      connection.auth_challenge AuthChallenge
daemon → agent admin      connection.auth         AuthResponse (Ed25519)
agent admin → daemon      connection.accepted     ConnectionAccepted
daemon → agent admin      connection.heartbeat    HeartbeatPayload (every 15s)
agent admin → daemon      (response)              HeartbeatAckPayload
```

The endpoint supports two enrollment modes:

- **Pairing.** The upgrade carries no machine credential and
  `connection.hello` includes the optional `pairing_code`. The admin validates
  the single-use code, verifies the challenge signature and pins the public key.
  `connection.accepted` returns `machine_token` (an `afm_<64 hex>` per-machine
  token; only its hash is stored) and `org_id`. The code is consumed atomically
  only after the signature and rotation checks succeed.
- **Token.** The upgrade carries `Authorization: Bearer <machine-token>`, where
  the token is the per-machine token from a previous pairing response — never
  the REST admin token. A pinned machine that presents no token is rejected; an
  unpaired machine is rejected too.

The admin pins a public key only after a valid signature, never from the
unauthenticated `hello`. A later key mismatch is accepted only with a rotation
proof in `connection.auth`: the old key signs
`agent-fleet-key-rotation-v1\nmachine_id=<id>\nnew_public_key=<b64>\nnonce=<b64>`
and the new key signs the challenge. Otherwise the admin sends a `connection`
`error` frame with `machine_key_mismatch` (also used when a token is bound to a
different machine id) before closing, so the daemon can stop with actionable
guidance instead of reconnecting forever.
The daemon stores the token at `<key_file>.token` (0600); it is not written to
the daemon TOML.

Authentication error codes:

| Code | Meaning |
|---|---|
| `machine_not_paired` | no pinned key and no valid pairing code or token |
| `machine_token_required` | the key is pinned but the upgrade presented no token |
| `pairing_code_invalid` | unknown, expired, already used or attempt-exhausted code, or a rejected challenge signature |
| `machine_key_mismatch` | the key changed without a valid rotation proof |
| `machine_org_mismatch` | the pairing code belongs to a different organization |
| `forbidden` | the machine id has been revoked |

Any JSON-RPC-like method can then flow both ways. The agent admin sends
`request` frames; the daemon answers with `response` or `error`.

### Methods

| Method | Direction | Purpose |
|---|---|---|
| `agent.providers.list` | CP → daemon | providers + capabilities |
| `agent.sessions.create` | CP → daemon | start a session |
| `agent.sessions.list` / `.get` | CP → daemon | session metadata/history |
| `agent.sessions.send_message` | CP → daemon | user input |
| `agent.sessions.interrupt` / `.resume` / `.shutdown` | CP → daemon | lifecycle |
| `agent.permissions.list` / `.respond` | CP → daemon | approvals |
| `workspace.list` / `.register` / `.remove` | CP → daemon | registry |
| `workspace.tree.list` | CP → daemon | lazy directory listing |
| `workspace.file.read` | CP → daemon | file content + ETag |
| `workspace.file.search` | CP → daemon | text search |
| `git.status` / `.diff` / `.diff.summary` / `.branches` / `.log` | CP → daemon | git read operations |
| `git.stage` / `.unstage` / `.commit` / `.checkout` | CP → daemon | authorized git writes |
| `health.get` | CP → daemon | version/uptime |
| `events.ack` | CP → daemon | acknowledge one applied durable outbox sequence |

`StreamFrame` (`open`/`chunk`/`end`/`cancel`) is defined for future large
payloads; the MVP sends file contents as a single response bounded by the
response cap.

### Events (daemon → agent admin)

Envelope `type` is `event`; `event` names:

- `agent.event` — payload is a `protocol::agent::AgentEvent`
- `workspace.event` — payload is a `protocol::workspace::WorkspaceEvent`
- `git.status.changed` — payload is `{ workspace_id, status: GitStatus }`
- `machine.status` — payload is the full `Machine` snapshot
- `connection.state` — gateway-generated `{ machine_id, connection_id, state }`
- `platform.lagged` — agent-admin notice `{ skipped, message }` when the
  in-process event buffer overflowed; clients should refetch current state

Daemon-originated events carry the daemon's event `sequence`, which is
monotonic while the daemon process lives and does not reset on a reconnect.
Control-plane-generated notices (including `platform.lagged`) use sequence `0`.
Clients should treat `sequence` as advisory ordering, deduplicate with
`event_id`, and treat a gap plus `platform.lagged` as a signal to refetch.

Durable `agent.event` frames also carry `cursor`, the event's **own outbox
sequence**. The admin applies the event and calls `events.ack` with that exact
sequence. Acknowledgements may arrive out of order after a replay: the storage
keeps acknowledged sequences above a gap and advances the durable cursor only
over the contiguous acknowledged prefix. Once the prefix advances, acknowledged
outbox rows are pruned. If a live frame is lost (broadcast lag, disconnect,
admin restart), its sequence is never acknowledged and the daemon replays it
until it is. Delivery is therefore at-least-once: consumers deduplicate with
`event_id`, and replay is idempotent. Ephemeral streaming deltas
(`message.delta`, `tool.input_delta`, `agent.raw_event`, `message.started`) are
not persisted and are not replayed; the completion event restores the final
content.

## 2. Unified agent protocol

Native provider events are parsed and mapped. The wire types are
`AgentEvent`, `SessionStatus`, `AgentToolCall`, `AgentPermissionRequest`.
Event names are stable: `session.*`, `turn.*`, `message.*`, `tool.*`,
`permission.*`, `agent.status.changed`, `agent.raw_event`.

Adapters preserve the native event in `raw_event` for debugging but the UI
only consumes the unified payload.

## 3. Browser API

REST under `/api/v1` plus `WS /api/v1/events`. See
[`crate-interfaces.md`](crate-interfaces.md) §12 for the route table and
[`architecture.md`](architecture.md) §13 for semantics.

Browser events are `PlatformEvent`:

```json
{
  "event_id": "evt_...",
  "sequence": 42,
  "type": "agent.event",
  "timestamp": "2026-11-13T10:00:00Z",
  "machine_id": "m_...",
  "session_id": "session_...",
  "payload": { }
}
```
