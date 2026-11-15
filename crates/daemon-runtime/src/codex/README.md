# Codex app-server adapter

This adapter launches:

```bash
codex app-server --listen stdio://
```

and speaks JSON-RPC 2.0 over JSONL (one JSON object per line). The protocol
below was grounded against **codex-cli 0.116.0** using:

```bash
codex app-server generate-json-schema --out /tmp/codex-schema
```

## Client requests

| Method | Params (used subset) | Purpose |
|---|---|---|
| `initialize` | `clientInfo`, `capabilities` | negotiate protocol |
| `thread/start` | `cwd`, `model`, `developerInstructions`, `approvalPolicy: "on-request"` | create a thread |
| `thread/resume` | `threadId`, `cwd`, `model` | resume an existing native thread |
| `turn/start` | `threadId`, `input: [{type:"text", text}]` | send a user message |
| `turn/interrupt` | `threadId`, `turnId` | interrupt the active turn |

Request ids are monotonically increasing integers. Responses are correlated by
id; notifications (`method` without `id`) update unified state; server requests
(`id` + `method`) are handled as approvals.

## Server notifications

| Method | Unified mapping |
|---|---|
| `thread/started` | native thread id (`provider_session_id` metadata) |
| `thread/status/changed` | `agent.status.changed` (`active`/`idle`/`systemError`) |
| `turn/started` | `turn.started` |
| `turn/completed` | `turn.completed`, `turn.interrupted` or `turn.failed` |
| `item/started` | `message.started` / `tool.started` |
| `item/completed` | `message.completed` / `tool.completed` / `tool.failed` |
| `item/agentMessage/delta` | `message.delta` |
| anything else | `agent.raw_event` |

## Server requests (approvals)

| Method | Response decision |
|---|---|
| `item/commandExecution/requestApproval` | `{"decision":"accept"\|"decline"}` |
| `item/fileChange/requestApproval` | `{"decision":"accept"\|"decline"}` |
| `execCommandApproval` | `{"decision":"approved"\|"denied"}` |
| `applyPatchApproval` | `{"decision":"approved"\|"denied"}` |

Approval prompts are registered with `AgentEventEmitter::request_permission`;
the runtime resolves the decision and emits `permission.approved` or
`permission.rejected` exactly once. The adapter then writes the JSON-RPC
response back to the app server.

Fixtures live in `fixtures/`. Tests never invoke the real CLI.
