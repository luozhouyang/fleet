/**
 * Presentation helpers for tool calls.
 *
 * Providers name their tools differently but the arguments share a few shapes.
 * This module turns a call into a verb, a target and an optional inline diff,
 * so the transcript can render one line per tool instead of dumping JSON.
 */

import { ToolCallStatus, type AgentToolCall } from '@agent-fleet/protocol-types';

/** How a tool call is presented. */
export type ToolKind =
  'bash' | 'edit' | 'write' | 'read' | 'search' | 'list' | 'web' | 'task' | 'other';

/** A one-line summary of a tool call. */
export interface ToolSummary {
  kind: ToolKind;
  /** A short verb, e.g. `Ran`, `Edited`, `Searched`. */
  verb: string;
  /** The most relevant argument: a command, path, pattern or URL. */
  target: string;
  /** A short detail for the expanded view, when the target is not the whole story. */
  detail?: string;
}

/** One line of a rendered inline diff. */
export interface DiffLineView {
  kind: 'add' | 'remove' | 'context' | 'header';
  text: string;
}

/** A parsed diff for edit-style tool calls. */
export interface InlineDiff {
  path?: string;
  lines: DiffLineView[];
  /** Whether the diff was cut to keep the card bounded. */
  truncated: boolean;
}

/** Maximum diff lines rendered inline. */
export const MAX_INLINE_DIFF_LINES = 400;

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' ? (value as Record<string, unknown>) : {};
}

function firstString(record: Record<string, unknown>, keys: string[]): string | undefined {
  for (const key of keys) {
    const value = record[key];
    if (typeof value === 'string' && value.trim()) return value;
  }
  return undefined;
}

const BASH_NAMES = ['bash', 'shell', 'exec', 'run', 'command', 'terminal'];
const EDIT_NAMES = ['edit', 'str_replace', 'apply_patch', 'patch', 'replace', 'multiedit'];
const WRITE_NAMES = ['write', 'create', 'write_file', 'save'];
const READ_NAMES = ['read', 'view', 'cat', 'open'];
const SEARCH_NAMES = ['grep', 'search', 'find', 'rg', 'glob'];
const LIST_NAMES = ['list', 'ls', 'tree'];
const WEB_NAMES = ['fetch', 'webfetch', 'browse', 'http'];
const TASK_NAMES = ['task', 'agent', 'todo', 'plan'];

function classify(name: string): ToolKind {
  const lower = name.toLowerCase();
  const matches = (candidates: string[]) =>
    candidates.some((candidate) => lower.includes(candidate));
  if (matches(BASH_NAMES)) return 'bash';
  if (matches(EDIT_NAMES)) return 'edit';
  if (matches(WRITE_NAMES)) return 'write';
  if (matches(SEARCH_NAMES)) return 'search';
  if (matches(READ_NAMES)) return 'read';
  if (matches(LIST_NAMES)) return 'list';
  if (matches(WEB_NAMES)) return 'web';
  if (matches(TASK_NAMES)) return 'task';
  return 'other';
}

const VERBS: Record<ToolKind, string> = {
  bash: 'Ran',
  edit: 'Edited',
  write: 'Wrote',
  read: 'Read',
  search: 'Searched',
  list: 'Listed',
  web: 'Fetched',
  task: 'Updated',
  other: 'Used',
};

/** Summarize a tool call for its one-line row. */
export function summarizeTool(tool: AgentToolCall): ToolSummary {
  const kind = classify(tool.name);
  const input = asRecord(tool.input);
  const verb = VERBS[kind];
  switch (kind) {
    case 'bash':
      return {
        kind,
        verb,
        target: firstString(input, ['command', 'cmd', 'script', 'args']) ?? tool.name,
      };
    case 'edit':
    case 'write':
      return {
        kind,
        verb,
        target:
          firstString(input, ['file_path', 'path', 'file', 'filename', 'target_file']) ?? tool.name,
      };
    case 'search':
      return {
        kind,
        verb,
        target: firstString(input, ['pattern', 'query', 'regex', 'glob']) ?? tool.name,
        detail: firstString(input, ['path', 'directory', 'glob']),
      };
    case 'read':
    case 'list':
      return {
        kind,
        verb,
        target: firstString(input, ['path', 'directory', 'file_path', 'file']) ?? tool.name,
      };
    case 'web':
      return {
        kind,
        verb,
        target: firstString(input, ['url', 'query']) ?? tool.name,
      };
    case 'task':
      return {
        kind,
        verb,
        target: firstString(input, ['description', 'title', 'content']) ?? tool.name,
      };
    default:
      return { kind, verb, target: tool.name };
  }
}

/** Whether the kind renders an inline diff when one can be parsed. */
export function kindShowsDiff(kind: ToolKind): boolean {
  return kind === 'edit' || kind === 'write';
}

/**
 * Parse the diff out of an edit tool call or its textual output.
 *
 * Handles unified diffs (`@@` hunks) and the old/new string pairs most
 * providers use. Returns `null` when neither is present, so the caller falls
 * back to plain output.
 */
export function diffFromTool(tool: AgentToolCall): InlineDiff | null {
  const input = asRecord(tool.input);
  const path = firstString(input, ['file_path', 'path', 'file', 'filename', 'target_file']);

  const patch = firstString(input, ['patch', 'diff']);
  if (patch) return diffFromUnified(patch, path, tool.output);

  const oldText = firstString(input, ['old_string', 'old_str', 'old_text', 'old']);
  const newText = firstString(input, ['new_string', 'new_str', 'new_text', 'new']);
  if (oldText !== undefined || newText !== undefined) {
    const lines: DiffLineView[] = [];
    for (const line of (oldText ?? '').split('\n')) {
      if (line) lines.push({ kind: 'remove', text: line });
    }
    for (const line of (newText ?? '').split('\n')) {
      if (line) lines.push({ kind: 'add', text: line });
    }
    return bounded({ path, lines });
  }

  // Other write tools report a unified diff as their output.
  if (typeof tool.output === 'string' && tool.output.includes('\n@@')) {
    return diffFromUnified(tool.output, path);
  }
  return null;
}

function diffFromUnified(
  text: string,
  path: string | undefined,
  output?: unknown,
): InlineDiff | null {
  const raw = text;
  const lines: DiffLineView[] = [];
  let sawHunk = false;
  for (const line of raw.split('\n')) {
    if (line.startsWith('@@')) {
      sawHunk = true;
      lines.push({ kind: 'header', text: line });
      continue;
    }
    if (line.startsWith('+++') || line.startsWith('---') || line.startsWith('diff ')) {
      continue;
    }
    if (line.startsWith('+')) lines.push({ kind: 'add', text: line.slice(1) });
    else if (line.startsWith('-')) lines.push({ kind: 'remove', text: line.slice(1) });
    else lines.push({ kind: 'context', text: line.startsWith(' ') ? line.slice(1) : line });
  }
  if (!sawHunk && lines.length === 0) {
    if (typeof output === 'string' && output.trim()) {
      return bounded({
        path,
        lines: output
          .split('\n')
          .filter(Boolean)
          .map((value) => ({ kind: 'add' as const, text: value })),
      });
    }
    return null;
  }
  return bounded({ path, lines });
}

function bounded(diff: { path?: string; lines: DiffLineView[] }): InlineDiff {
  if (diff.lines.length <= MAX_INLINE_DIFF_LINES) {
    return { ...diff, truncated: false };
  }
  return {
    path: diff.path,
    lines: diff.lines.slice(0, MAX_INLINE_DIFF_LINES),
    truncated: true,
  };
}

/** Whether a tool call ended in failure. */
export function toolFailed(tool: AgentToolCall): boolean {
  return tool.status === ToolCallStatus.Failed || tool.status === ToolCallStatus.Cancelled;
}

/** Best-effort text of a tool's output. */
export function outputText(output: unknown, maxLength = 2000): string {
  if (output === undefined || output === null) return '';
  if (typeof output === 'string') return clip(output, maxLength);
  if (typeof output === 'object' && 'content' in (output as Record<string, unknown>)) {
    const content = (output as Record<string, unknown>).content;
    if (typeof content === 'string') return clip(content, maxLength);
  }
  try {
    return clip(JSON.stringify(output, null, 2), maxLength);
  } catch {
    return clip(String(output), maxLength);
  }
}

function clip(value: string, maxLength: number): string {
  return value.length > maxLength ? `${value.slice(0, maxLength)}…` : value;
}
