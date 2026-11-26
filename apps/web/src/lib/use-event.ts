import { useCallback, useRef } from 'react';

/**
 * Returns a callback with a stable identity that always invokes the latest
 * version of `fn`.
 *
 * Useful for handing handlers to memoized transcript rows: the row keeps its
 * memoization, while the handler still closes over current state.
 */
export function useEvent<Args extends unknown[], Return>(
  fn: (...args: Args) => Return,
): (...args: Args) => Return {
  const ref = useRef(fn);
  ref.current = fn;
  return useCallback((...args: Args) => ref.current(...args), []);
}
