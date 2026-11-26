import { useCallback, useEffect, useRef, useState } from 'react';
import { readJson, removePersisted, writeJson } from '@/lib/persist/storage';
import {
  draftKey,
  historyKey,
  pushHistory,
  recallNext,
  recallPrevious,
} from '@/lib/session/composer-model';

/** Load a session's unsent draft. */
export function readDraft(sessionId: string): string {
  return readJson<string>(draftKey(sessionId), '');
}

/** Persist a session's draft; an empty draft removes the key. */
export function writeDraft(sessionId: string, value: string): void {
  if (!value) {
    removePersisted(draftKey(sessionId));
    return;
  }
  writeJson(draftKey(sessionId), value);
}

/** Load a session's prompt history. */
export function readHistory(sessionId: string): string[] {
  const value = readJson<string[]>(historyKey(sessionId), []);
  return Array.isArray(value) ? value.filter((entry) => typeof entry === 'string') : [];
}

/** Append a sent prompt to the session's history. */
export function recordHistory(sessionId: string, prompt: string): string[] {
  const next = pushHistory(readHistory(sessionId), prompt);
  writeJson(historyKey(sessionId), next);
  return next;
}

/**
 * Draft state bound to the session's persisted text.
 *
 * Writes are debounced: a keystroke should not hit localStorage on every
 * character, but leaving the component flushes the latest value.
 */
export function useDraft(sessionId: string): {
  draft: string;
  setDraft: (value: string) => void;
  clearDraft: () => void;
} {
  const [draft, setDraftState] = useState(() => readDraft(sessionId));
  const draftRef = useRef(draft);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    setDraftState(readDraft(sessionId));
  }, [sessionId]);

  const flush = useCallback(() => {
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
    writeDraft(sessionId, draftRef.current);
  }, [sessionId]);

  useEffect(() => {
    const handle = () => flush();
    window.addEventListener('pagehide', handle);
    return () => {
      window.removeEventListener('pagehide', handle);
      flush();
    };
  }, [flush]);

  const setDraft = useCallback(
    (value: string) => {
      draftRef.current = value;
      setDraftState(value);
      if (timer.current !== null) window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => {
        timer.current = null;
        writeDraft(sessionId, value);
      }, 300);
    },
    [sessionId],
  );

  const clearDraft = useCallback(() => {
    draftRef.current = '';
    setDraftState('');
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
    removePersisted(draftKey(sessionId));
  }, [sessionId]);

  return { draft, setDraft, clearDraft };
}

/** Prompt history with a navigation cursor. */
export function usePromptHistory(sessionId: string): {
  history: string[];
  record: (prompt: string) => void;
  /** Previous entry relative to the cursor, or `null` at the oldest entry. */
  previous: () => string | null;
  /** Next entry, or `''` when stepping past the newest one. */
  next: () => string | null;
  /** Whether the user is currently walking history. */
  recalling: boolean;
  resetCursor: () => void;
} {
  const [history, setHistory] = useState<string[]>(() => readHistory(sessionId));
  const cursor = useRef<number | null>(null);

  useEffect(() => {
    setHistory(readHistory(sessionId));
    cursor.current = null;
  }, [sessionId]);

  const record = useCallback(
    (prompt: string) => {
      setHistory(recordHistory(sessionId, prompt));
      cursor.current = null;
    },
    [sessionId],
  );

  const previous = useCallback(() => {
    const step = recallPrevious(history, { index: cursor.current });
    if (!step) return null;
    cursor.current = step.cursor.index;
    return step.value;
  }, [history]);

  const next = useCallback(() => {
    const step = recallNext(history, { index: cursor.current });
    if (!step) return null;
    cursor.current = step.cursor.index;
    return step.value;
  }, [history]);

  const resetCursor = useCallback(() => {
    cursor.current = null;
  }, []);

  // The cursor is a ref, so this boolean reflects the latest step.
  return { history, record, previous, next, recalling: cursor.current !== null, resetCursor };
}
