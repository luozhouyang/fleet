# Claude Code stream-json fixtures

These fixtures are anonymized samples of Claude Code CLI output in
`--output-format stream-json` mode.

- CLI version: **2.1.319 (Claude Code)**
- Invocation: `claude -p --input-format stream-json --output-format stream-json --verbose`
- Input is one JSON object per line on stdin:
  `{"type":"user","message":{"role":"user","content":[{"type":"text","text":"..."}]}}`

Files:

| File | Covers |
|---|---|
| `init_system.jsonl` | `system` / `init` line with the native session id |
| `assistant_text.jsonl` | assistant text message + successful `result` |
| `tool_use.jsonl` | assistant text + `tool_use`, user `tool_result`, final answer |
| `error_result.jsonl` | `result` with `is_error: true` |
| `partial_delta.jsonl` | `stream_event` partial text deltas |

All paths, session ids, tool ids and message ids are synthetic. Tests must not
invoke the real CLI. Refresh these fixtures when the upstream protocol changes.
