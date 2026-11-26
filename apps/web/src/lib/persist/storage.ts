/**
 * Small persistence helpers for browser-local drafts and prompt history.
 *
 * Everything here degrades to an in-memory map when `localStorage` is
 * unavailable (private mode, disabled storage), so a denied write never breaks
 * composing a message.
 */

const memory = new Map<string, string>();

function storage(): Storage | null {
  try {
    const candidate = window.localStorage;
    const probe = '__fleet_probe__';
    candidate.setItem(probe, '1');
    candidate.removeItem(probe);
    return candidate;
  } catch {
    return null;
  }
}

/** Read a value, preferring persisted storage. */
export function readPersisted(key: string): string | null {
  const store = storage();
  if (store) {
    try {
      return store.getItem(key);
    } catch {
      return memory.get(key) ?? null;
    }
  }
  return memory.get(key) ?? null;
}

/** Write a value; `null` removes it. */
export function writePersisted(key: string, value: string | null): void {
  const store = storage();
  if (value === null) {
    memory.delete(key);
    try {
      store?.removeItem(key);
    } catch {
      // Nothing else to do; the in-memory copy is already gone.
    }
    return;
  }
  memory.set(key, value);
  try {
    store?.setItem(key, value);
  } catch {
    // Quota or permission failures keep the in-memory copy only.
  }
}

/** Read and parse JSON, returning `fallback` on absence or corruption. */
export function readJson<T>(key: string, fallback: T): T {
  const raw = readPersisted(key);
  if (raw === null) return fallback;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return fallback;
  }
}

/** Serialize and write JSON. */
export function writeJson(key: string, value: unknown): void {
  try {
    writePersisted(key, JSON.stringify(value));
  } catch {
    // A cyclic value is a programming error, not a user-facing failure.
  }
}

/** Remove a key. */
export function removePersisted(key: string): void {
  writePersisted(key, null);
}

/** Test helper: drop every in-memory value. */
export function clearMemoryStore(): void {
  memory.clear();
}
