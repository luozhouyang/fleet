# Protocols

Three protocols meet in this platform. Types are defined once in
`crates/protocol`; everything else consumes them.

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
daemon → control plane      connection.hello        HelloPayload
control plane → daemon      connection.auth_challenge AuthChallenge
daemon → control plane      connection.auth         AuthResponse (Ed25519)
control plane → daemon      connection.accepted     ConnectionAccepted
daemon → control plane      connection.heartbeat    HeartbeatPayload (every 15s)
control plane → daemon      (response)              HeartbeatAckPayload
```

The WebSocket upgrade request must carry `Authorization: Bearer <token>`.
The control plane pins the first public key seen for a machine id. A later
mismatch is accepted only with a rotation proof in `connection.auth`: the old
key signs `agent-fleet-key-rotation-v1\nmachine_id=<id>\nnew_public_key=<b64>\nnonce=<b64>`
and the new key signs the challenge. Otherwise the connection is rejected.

The control plane pins the first public key seen for a machine id; later
mismatches are rejected.

Any JSON-RPC-like method can then flow both ways. The control plane sends
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
| `health.get` | CP → daemon | version/uptime |

Large payloads use `StreamFrame` (`open`/`chunk`/`end`/`cancel`) with base64
chunks; the MVP sends file contents as a single response when they fit the
response cap.

### Events (daemon → control plane)

Envelope `type` is `event`; `event` names:

- `agent.event` — payload is a `protocol::agent::AgentEvent`
- `workspace.event` — payload is a `protocol::workspace::WorkspaceEvent`
- `git.status.changed` — payload is a `GitStatus`
- `machine.status` — payload is `MachineStatusEvent`
- `connection.closed` — gateway-generated

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
