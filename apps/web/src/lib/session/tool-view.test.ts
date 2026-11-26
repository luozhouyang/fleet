import { ToolCallStatus, type AgentToolCall } from '@agent-fleet/protocol-types';
import { describe, expect, it } from 'vitest';
import {
  MAX_INLINE_DIFF_LINES,
  diffFromTool,
  kindShowsDiff,
  outputText,
  summarizeTool,
  toolFailed,
} from './tool-view';

function tool(
  name: string,
  input: unknown,
  output?: unknown,
  status = ToolCallStatus.Completed,
): AgentToolCall {
  return {
    id: 't_1',
    session_id: 's_1',
    name,
    status,
    input,
    output,
    started_at: '2026-01-01T10:00:00Z',
  };
}

describe('tool summary', () => {
  it('reads the command out of a shell tool', () => {
    const summary = summarizeTool(tool('Bash', { command: 'cargo test -p web' }));
    expect(summary).toMatchObject({ kind: 'bash', verb: 'Ran', target: 'cargo test -p web' });
  });

  it('reads the path out of an edit tool regardless of provider naming', () => {
    const summary = summarizeTool(
      tool('str_replace_editor', { file_path: 'src/main.ts', old_string: 'a' }),
    );
    expect(summary).toMatchObject({ kind: 'edit', target: 'src/main.ts' });
  });

  it('falls back to the tool name for unknown shapes', () => {
    const summary = summarizeTool(tool('mcp__custom__do_thing', { anything: 1 }));
    expect(summary).toMatchObject({ kind: 'other', verb: 'Used', target: 'mcp__custom__do_thing' });
  });

  it('orders classification so write does not shadow search', () => {
    expect(summarizeTool(tool('grep', { pattern: 'TODO' })).kind).toBe('search');
    expect(summarizeTool(tool('write_file', { path: 'a.txt' })).kind).toBe('write');
  });
});

describe('inline diffs', () => {
  it('parses an old/new pair into remove then add lines', () => {
    const diff = diffFromTool(
      tool('Edit', { file_path: 'a.ts', old_string: 'const a = 1;', new_string: 'const a = 2;' }),
    );
    expect(diff?.path).toBe('a.ts');
    expect(diff?.lines).toEqual([
      { kind: 'remove', text: 'const a = 1;' },
      { kind: 'add', text: 'const a = 2;' },
    ]);
  });

  it('parses a unified diff and skips file headers', () => {
    const patch = ['--- a/x', '+++ b/x', '@@ -1,2 +1,2 @@', '-old', '+new', ' ctx'].join('\n');
    const diff = diffFromTool(tool('apply_patch', { patch }));
    expect(diff?.lines.map((line) => line.kind)).toEqual(['header', 'remove', 'add', 'context']);
    expect(diff?.lines[2].text).toBe('new');
  });

  it('bounds long diffs', () => {
    const newString = Array.from(
      { length: MAX_INLINE_DIFF_LINES + 20 },
      (_, i) => `line ${i}`,
    ).join('\n');
    const diff = diffFromTool(tool('Edit', { path: 'big.ts', new_string: newString }));
    expect(diff?.truncated).toBe(true);
    expect(diff?.lines).toHaveLength(MAX_INLINE_DIFF_LINES);
  });

  it('returns null when nothing diff-like exists', () => {
    expect(diffFromTool(tool('Bash', { command: 'ls' }, 'a.txt'))).toBeNull();
  });

  it('only renders diffs for edit-style kinds', () => {
    expect(kindShowsDiff('edit')).toBe(true);
    expect(kindShowsDiff('write')).toBe(true);
    expect(kindShowsDiff('bash')).toBe(false);
  });
});

describe('tool output', () => {
  it('clips long text', () => {
    expect(outputText('abcdef', 3)).toBe('abc…');
    expect(outputText(undefined)).toBe('');
    expect(outputText({ content: 'hello' })).toBe('hello');
    expect(outputText({ stdout: 'x' })).toBe('{\n  "stdout": "x"\n}');
  });

  it('treats failure and cancellation as failed', () => {
    expect(toolFailed(tool('Bash', {}, undefined, ToolCallStatus.Failed))).toBe(true);
    expect(toolFailed(tool('Bash', {}, undefined, ToolCallStatus.Cancelled))).toBe(true);
    expect(toolFailed(tool('Bash', {}))).toBe(false);
  });
});
