/**
 * Pure composer rules.
 *
 * Attachment limits, draft keys, prompt history, the slash-command catalog and
 * mention parsing all live here so they can be tested without React.
 */

import type { Attachment } from '@agent-fleet/protocol-types';

/** Maximum attachments on one message. */
export const MAX_ATTACHMENTS = 4;
/** Maximum bytes for one image attachment (before base64 expansion). */
export const MAX_IMAGE_BYTES = 256 * 1024;
/** Maximum bytes for one text attachment. */
export const MAX_TEXT_BYTES = 64 * 1024;
/**
 * Where the recall cursor sits while the user walks history with the arrows.
 *
 * The caret is not a good signal here: after a recalled prompt the caret is at
 * its end, and the next ArrowUp must still mean "older prompt" rather than
 * "move the caret up one line". `index === null` means recall is inactive.
 */
export interface HistoryCursor {
  index: number | null;
}

/** Step the recall cursor one entry older. */
export function recallPrevious(
  history: string[],
  cursor: HistoryCursor,
): { value: string; cursor: HistoryCursor } | null {
  if (history.length === 0) return null;
  const index = cursor.index === null ? history.length - 1 : cursor.index - 1;
  if (index < 0) return null;
  return { value: history[index], cursor: { index } };
}

/** Step the recall cursor one entry newer; past the newest it returns `''`. */
export function recallNext(
  history: string[],
  cursor: HistoryCursor,
): { value: string; cursor: HistoryCursor } | null {
  if (cursor.index === null) return null;
  const index = cursor.index + 1;
  if (index >= history.length) return { value: '', cursor: { index: null } };
  return { value: history[index], cursor: { index } };
}

/** Prompt history entries kept per session. */
export const MAX_HISTORY = 50;

export interface PendingAttachment {
  id: string;
  name: string;
  mimeType: string;
  size: number;
  /** Base64 payload without a data-URL prefix. */
  dataBase64: string;
  /** Object URL for image previews; revoked when the attachment is removed. */
  previewUrl?: string;
}

function isImage(mimeType: string): boolean {
  return mimeType.startsWith('image/');
}

function isTextual(file: { name: string; type: string }): boolean {
  if (file.type.startsWith('text/')) return true;
  return /\.(md|markdown|json|ya?ml|toml|rs|ts|tsx|js|jsx|css|html|sh|py|go|sql|txt|log|csv)$/i.test(
    file.name,
  );
}

/** Validation outcome for one file, before any reading happens. */
export function checkAttachment(
  file: { name: string; type: string; size: number },
  existing: number,
  imagesSupported: boolean,
): { ok: true; image: boolean } | { ok: false; error: string } {
  if (existing >= MAX_ATTACHMENTS) {
    return { ok: false, error: `Attach at most ${MAX_ATTACHMENTS} files.` };
  }
  const image = isImage(file.type);
  if (image && !imagesSupported) {
    return { ok: false, error: 'This agent does not accept image attachments.' };
  }
  if (!image && !isTextual(file)) {
    return { ok: false, error: `${file.name} is not an image or a text file.` };
  }
  const limit = image ? MAX_IMAGE_BYTES : MAX_TEXT_BYTES;
  if (file.size > limit) {
    return {
      ok: false,
      error: `${file.name} is larger than ${Math.round(limit / 1024)} KiB.`,
    };
  }
  return { ok: true, image };
}

/** Convert pending attachments to the wire shape. */
export function toWireAttachments(attachments: PendingAttachment[]): Attachment[] {
  return attachments.map((attachment) => ({
    name: attachment.name,
    mime_type: attachment.mimeType,
    data_base64: attachment.dataBase64,
  }));
}

/** Storage key for a session's unsent draft. */
export function draftKey(sessionId: string): string {
  return `fleet.draft.${sessionId}`;
}

/** Storage key for a session's prompt history. */
export function historyKey(sessionId: string): string {
  return `fleet.prompt-history.${sessionId}`;
}

/** Append a prompt to history, dropping the oldest entries. */
export function pushHistory(history: string[], prompt: string): string[] {
  const trimmed = prompt.trim();
  if (!trimmed) return history;
  const withoutDuplicate = history.filter((entry) => entry !== trimmed);
  return [...withoutDuplicate, trimmed].slice(-MAX_HISTORY);
}

export interface SlashCommand {
  name: string;
  description: string;
  /** Whether the command needs the session to be active. */
  requiresActive: boolean;
}

/** Client-side commands offered by the `/` picker. */
export const SLASH_COMMANDS: SlashCommand[] = [
  { name: 'interrupt', description: 'Stop the current turn', requiresActive: true },
  { name: 'resume', description: 'Resume an interrupted session', requiresActive: false },
  { name: 'close', description: 'Close this session', requiresActive: false },
  { name: 'attach', description: 'Attach a file to the next message', requiresActive: true },
  { name: 'help', description: 'Show composer shortcuts', requiresActive: false },
];

/** Commands matching a `/` prefix, best match first. */
export function filterCommands(input: string): SlashCommand[] {
  if (!input.startsWith('/')) return [];
  const query = input.slice(1).toLowerCase();
  if (query.includes(' ')) return [];
  return SLASH_COMMANDS.filter((command) => command.name.startsWith(query));
}

/** The active mention query at the caret, or `null`. */
export function mentionQuery(
  value: string,
  caret: number,
): { query: string; start: number } | null {
  const before = value.slice(0, caret);
  const at = before.lastIndexOf('@');
  if (at < 0) return null;
  // A mention starts at the beginning of the text or after whitespace.
  if (at > 0 && !/\s/.test(before[at - 1])) return null;
  const query = before.slice(at + 1);
  // Paths may contain `/` and `.` but not spaces; stop at the first one.
  if (/\s/.test(query)) return null;
  return { query, start: at };
}

/**
 * Replace the active mention with the chosen path.
 *
 * Returns the new value and caret position so the textarea keeps the caret
 * after the inserted path.
 */
export function applyMention(
  value: string,
  mention: { start: number },
  path: string,
): { value: string; caret: number } {
  const before = value.slice(0, mention.start);
  const after = value.slice(mention.start).replace(/^@[^\s]*/, '');
  const inserted = `@${path} `;
  return { value: `${before}${inserted}${after}`, caret: before.length + inserted.length };
}

/** Paths mentioned in a prompt, in order of appearance. */
export function mentionedPaths(value: string): string[] {
  const paths: string[] = [];
  const pattern = /(^|\s)@([^\s]+)/g;
  let match = pattern.exec(value);
  while (match) {
    paths.push(match[2]);
    match = pattern.exec(value);
  }
  return paths;
}

/** One queued prompt. */
export interface QueuedPrompt {
  id: string;
  content: string;
  attachments: PendingAttachment[];
  createdAt: number;
}

let queuedCounter = 0;

/** Create a queued prompt. */
export function newQueuedPrompt(content: string, attachments: PendingAttachment[]): QueuedPrompt {
  queuedCounter += 1;
  return {
    id: `q_${Date.now().toString(36)}_${queuedCounter}`,
    content,
    attachments,
    createdAt: Date.now(),
  };
}

/** Move an item within a list, returning a new list. */
export function moveItem<T>(items: T[], from: number, to: number): T[] {
  if (from === to || from < 0 || to < 0 || from >= items.length || to >= items.length) {
    return items;
  }
  const next = [...items];
  const [item] = next.splice(from, 1);
  next.splice(to, 0, item);
  return next;
}
