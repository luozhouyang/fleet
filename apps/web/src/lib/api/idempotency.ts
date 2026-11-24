/**
 * Create a key for one user action.
 *
 * Generate it once when the action starts and reuse it for every retry, so the
 * daemon replays the first response instead of executing the command twice.
 */
export function newIdempotencyKey(scope: string): string {
  const random =
    typeof crypto !== 'undefined' && 'randomUUID' in crypto
      ? crypto.randomUUID()
      : `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  return `${scope}:${random}`;
}
