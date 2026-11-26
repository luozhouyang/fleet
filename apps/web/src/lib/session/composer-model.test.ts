import { describe, expect, it } from 'vitest';
import {
  MAX_ATTACHMENTS,
  MAX_HISTORY,
  MAX_IMAGE_BYTES,
  MAX_TEXT_BYTES,
  checkAttachment,
  recallNext,
  recallPrevious,
  applyMention,
  draftKey,
  filterCommands,
  historyKey,
  mentionQuery,
  mentionedPaths,
  moveItem,
  pushHistory,
  transcriptContent,
} from './composer-model';

describe('draft keys', () => {
  it('namespaces drafts and history per session', () => {
    expect(draftKey('s_1')).toBe('fleet.draft.s_1');
    expect(historyKey('s_1')).toBe('fleet.prompt-history.s_1');
    expect(draftKey('s_1')).not.toBe(draftKey('s_2'));
  });
});

describe('prompt history', () => {
  it('appends and deduplicates', () => {
    expect(pushHistory(['a', 'b'], 'a')).toEqual(['b', 'a']);
  });

  it('ignores blank prompts', () => {
    expect(pushHistory(['a'], '   ')).toEqual(['a']);
  });

  it('keeps the newest entries within the cap', () => {
    let history: string[] = [];
    for (let index = 0; index < MAX_HISTORY + 5; index += 1) {
      history = pushHistory(history, `prompt ${index}`);
    }
    expect(history).toHaveLength(MAX_HISTORY);
    expect(history.at(-1)).toBe(`prompt ${MAX_HISTORY + 4}`);
  });
});

describe('slash commands', () => {
  it('filters by prefix and ignores text after a space', () => {
    expect(filterCommands('/').map((command) => command.name)).toContain('interrupt');
    expect(filterCommands('/re').map((command) => command.name)).toEqual(['resume']);
    expect(filterCommands('/resume now')).toEqual([]);
    expect(filterCommands('hello')).toEqual([]);
  });
});

describe('mentions', () => {
  it('finds a mention at the start or after whitespace', () => {
    expect(mentionQuery('@src', 4)).toEqual({ query: 'src', start: 0 });
    expect(mentionQuery('look at @src', 12)).toEqual({ query: 'src', start: 8 });
  });

  it('ignores email-like text and closed mentions', () => {
    expect(mentionQuery('mail a@b', 9)).toBeNull();
    expect(mentionQuery('@src/main.rs next', 17)).toBeNull();
  });

  it('replaces the mention and places the caret after it', () => {
    const value = 'see @src';
    const mention = mentionQuery(value, value.length);
    expect(mention).not.toBeNull();
    const applied = applyMention(value, mention!, 'src/main.rs');
    expect(applied.value).toBe('see @src/main.rs ');
    expect(applied.caret).toBe(applied.value.length);
  });

  it('extracts every mentioned path once per occurrence', () => {
    expect(mentionedPaths('read @a.txt and @src/b.rs')).toEqual(['a.txt', 'src/b.rs']);
    expect(mentionedPaths('mail a@b.com')).toEqual([]);
  });
});

describe('attachment validation', () => {
  const file = (name: string, type: string, size: number) => ({ name, type, size });

  it('accepts a small image when images are supported', () => {
    expect(checkAttachment(file('a.png', 'image/png', 1024), 0, true)).toEqual({
      ok: true,
      image: true,
    });
  });

  it('rejects images when the agent does not accept them', () => {
    const result = checkAttachment(file('a.png', 'image/png', 1024), 0, false);
    expect(result.ok).toBe(false);
  });

  it('accepts text files by mime type or extension', () => {
    expect(checkAttachment(file('a.txt', 'text/plain', 10), 0, false).ok).toBe(true);
    expect(checkAttachment(file('a.rs', '', 10), 0, false).ok).toBe(true);
    expect(checkAttachment(file('a.bin', 'application/octet-stream', 10), 0, false).ok).toBe(false);
  });

  it('enforces per-kind and total limits', () => {
    expect(checkAttachment(file('big.png', 'image/png', MAX_IMAGE_BYTES + 1), 0, true).ok).toBe(
      false,
    );
    expect(checkAttachment(file('big.txt', 'text/plain', MAX_TEXT_BYTES + 1), 0, false).ok).toBe(
      false,
    );
    expect(checkAttachment(file('a.txt', 'text/plain', 10), MAX_ATTACHMENTS, false).ok).toBe(false);
  });
});

describe('transcript content', () => {
  const attachment = (name: string, mime_type?: string) => ({
    name,
    mime_type,
    data_base64: 'AAAA',
  });

  it('returns the text unchanged without attachments', () => {
    expect(transcriptContent('hello', [])).toBe('hello');
  });

  it('appends attachment summaries in order and never the payload', () => {
    const content = transcriptContent('look', [
      attachment('shot.png', 'image/png'),
      attachment('notes.txt'),
    ]);
    expect(content).toBe('look\n[attachment: shot.png (image/png)]\n[attachment: notes.txt]');
    expect(content).not.toContain('AAAA');
  });

  it('stands alone for an attachment-only prompt', () => {
    expect(transcriptContent('', [attachment('shot.png', 'image/png')])).toBe(
      '[attachment: shot.png (image/png)]',
    );
  });
});

describe('history recall cursor', () => {
  const history = ['one', 'two', 'three'];

  it('walks backwards from the newest entry and stops at the oldest', () => {
    let cursor = { index: null as number | null };
    const first = recallPrevious(history, cursor);
    expect(first).toEqual({ value: 'three', cursor: { index: 2 } });
    cursor = first!.cursor;
    const second = recallPrevious(history, cursor);
    expect(second?.value).toBe('two');
    cursor = second!.cursor;
    expect(recallPrevious(history, cursor)?.value).toBe('one');
    expect(recallPrevious(history, { index: 0 })).toBeNull();
  });

  it('walks forwards and clears past the newest entry', () => {
    expect(recallNext(history, { index: 0 })?.value).toBe('two');
    expect(recallNext(history, { index: 2 })).toEqual({
      value: '',
      cursor: { index: null },
    });
  });

  it('is inactive until the first step', () => {
    expect(recallNext(history, { index: null })).toBeNull();
  });
});

describe('queue ordering', () => {
  it('moves an item within bounds', () => {
    expect(moveItem(['a', 'b', 'c'], 0, 2)).toEqual(['b', 'c', 'a']);
    expect(moveItem(['a', 'b'], 0, 5)).toEqual(['a', 'b']);
  });
});
