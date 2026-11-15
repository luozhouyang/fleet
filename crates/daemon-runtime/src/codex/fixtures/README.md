# Codex app-server fixtures

Anonymized samples of the Codex app-server protocol.

- CLI version: **codex-cli 0.116.0**
- Schema source: `codex app-server generate-json-schema --out /tmp/codex-schema`
- Transport: JSON-RPC 2.0 over stdio, one JSON object per line
- Launch: `codex app-server --listen stdio://`

Methods used by the adapter (verified against the generated schema):

| Direction | Method | Purpose |
|---|---|---|
| client → server | `initialize` | negotiate client info/capabilities |
| client → server | `thread/start` | create a conversation thread |
| client → server | `thread/resume` | resume an existing thread |
| client → server | `turn/start` | send a user text input |
| client → server | `turn/interrupt` | interrupt the active turn |
| server → client | `thread/started` | thread metadata, including its id |
| server → client | `turn/started` | turn started |
| server → client | `turn/completed` | turn completed/interrupted/failed |
| server → client | `item/started` | message/tool item started |
| server → client | `item/completed` | message/tool item completed |
| server → client | `item/agentMessage/delta` | assistant text streaming |
| server → client | `thread/status/changed` | thread runtime status |
| server → client (request) | `item/commandExecution/requestApproval` | command permission prompt |
| server → client (request) | `item/fileChange/requestApproval` | patch permission prompt |

Files:

| File | Covers |
|---|---|
| `thread_start.jsonl` | `initialize` and `thread/start` responses, `thread/started` |
| `turn_text.jsonl` | agent message streaming and turn completion |
| `tool_command.jsonl` | command execution item lifecycle |
| `error_turn.jsonl` | failed turn |
| `approval.jsonl` | server-initiated approval requests |
| `resume.jsonl` | `thread/resume` response |

Ids, paths and commands are synthetic. Tests must not invoke the real CLI.
