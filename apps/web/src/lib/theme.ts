/**
 * Light/dark/system theme.
 *
 * The chosen mode lives in `localStorage` and is applied as `data-theme` on
 * `<html>` before first paint by the inline script in `index.html`. This module
 * owns everything after that: reading the stored mode, switching it, and
 * telling subscribers (Monaco, for example) which theme is now active.
 */

export type ThemeMode = 'light' | 'dark' | 'system';
export type ResolvedTheme = 'light' | 'dark';

/** localStorage key shared with the pre-paint script in `index.html`. */
export const THEME_STORAGE_KEY = 'fleet.theme';

const DARK_QUERY = '(prefers-color-scheme: dark)';

/** The stored mode, defaulting to `system`. */
export function readThemeMode(): ThemeMode {
  try {
    const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
    if (stored === 'light' || stored === 'dark' || stored === 'system') return stored;
  } catch {
    // Storage can be unavailable (private mode, disabled cookies); the OS
    // preference is still a valid answer.
  }
  return 'system';
}

/** Whether the active palette is dark, accounting for the OS preference. */
export function resolveTheme(mode: ThemeMode = readThemeMode()): ResolvedTheme {
  if (mode === 'light' || mode === 'dark') return mode;
  return window.matchMedia(DARK_QUERY).matches ? 'dark' : 'light';
}

/** The `data-theme` attribute for a mode: absent for `system`. */
function attributeFor(mode: ThemeMode): string | null {
  return mode === 'system' ? null : mode;
}

/**
 * Apply a mode to the document.
 *
 * `animate` adds a short color transition so the swap does not flash; it is
 * skipped under reduced motion.
 */
export function applyThemeMode(mode: ThemeMode, options: { animate?: boolean } = {}): void {
  const root = document.documentElement;
  const attribute = attributeFor(mode);
  if (options.animate) {
    root.classList.add('theme-swapping');
    window.setTimeout(() => root.classList.remove('theme-swapping'), 200);
  }
  if (attribute) root.dataset.theme = attribute;
  else delete root.dataset.theme;

  const resolved = resolveTheme(mode);
  root.style.colorScheme = resolved;
  // Keep the browser chrome (mobile address bar) in step with the app.
  const meta = document.querySelector<HTMLMetaElement>('meta[name="theme-color"]');
  if (meta) meta.content = resolved === 'dark' ? '#17150F' : '#F4F1E8';
}

/** Persist and apply a mode. */
export function setThemeMode(mode: ThemeMode, options: { animate?: boolean } = {}): void {
  try {
    window.localStorage.setItem(THEME_STORAGE_KEY, mode);
  } catch {
    // A denied write only costs persistence, not the current session.
  }
  applyThemeMode(mode, options);
}

/** Cycles light → dark → system. */
export function nextThemeMode(mode: ThemeMode): ThemeMode {
  if (mode === 'light') return 'dark';
  if (mode === 'dark') return 'system';
  return 'light';
}

/**
 * Subscribe to resolved-theme changes from either the mode or the OS
 * preference. Returns an unsubscribe function.
 */
export function subscribeResolvedTheme(listener: (theme: ResolvedTheme) => void): () => void {
  const media = window.matchMedia(DARK_QUERY);
  let last = resolveTheme();
  const notify = () => {
    const theme = resolveTheme();
    if (theme === last) return;
    last = theme;
    listener(theme);
  };
  media.addEventListener('change', notify);
  window.addEventListener('fleet:theme-changed', notify);
  return () => {
    media.removeEventListener('change', notify);
    window.removeEventListener('fleet:theme-changed', notify);
  };
}

/** Announce a theme change to subscribers after applying it. */
export function notifyThemeChanged(): void {
  window.dispatchEvent(new Event('fleet:theme-changed'));
}
