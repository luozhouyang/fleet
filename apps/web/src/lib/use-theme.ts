import { useCallback, useEffect, useState } from 'react';
import {
  applyThemeMode,
  nextThemeMode,
  notifyThemeChanged,
  readThemeMode,
  resolveTheme,
  setThemeMode,
  subscribeResolvedTheme,
  type ResolvedTheme,
  type ThemeMode,
} from './theme';

/**
 * The active theme mode plus the resolved palette.
 *
 * The pre-paint script in `index.html` has already applied the stored mode, so
 * this only re-applies it (idempotent) and tracks changes from the toggle, the
 * OS preference, or another tab.
 */
export function useTheme(): {
  mode: ThemeMode;
  resolved: ResolvedTheme;
  setMode: (mode: ThemeMode) => void;
  cycle: () => void;
} {
  const [mode, setModeState] = useState<ThemeMode>(() => readThemeMode());
  const [resolved, setResolved] = useState<ResolvedTheme>(() => resolveTheme());

  useEffect(() => {
    applyThemeMode(mode);
  }, [mode]);

  useEffect(() => subscribeResolvedTheme(setResolved), []);

  // Another tab changing the stored mode must update this one too.
  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key !== 'fleet.theme') return;
      setModeState(readThemeMode());
    };
    window.addEventListener('storage', onStorage);
    return () => window.removeEventListener('storage', onStorage);
  }, []);

  const setMode = useCallback((next: ThemeMode) => {
    setThemeMode(next, { animate: true });
    notifyThemeChanged();
    setModeState(next);
    setResolved(resolveTheme(next));
  }, []);

  const cycle = useCallback(() => {
    setMode(nextThemeMode(readThemeMode()));
  }, [setMode]);

  return { mode, resolved, setMode, cycle };
}
