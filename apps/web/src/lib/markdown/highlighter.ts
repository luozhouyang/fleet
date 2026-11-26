/**
 * Syntax highlighting with Shiki.
 *
 * The highlighter is a core instance with a curated language set and the
 * JavaScript regex engine: no WASM fetch, and no bundled grammar the app does
 * not use. Shiki itself is imported dynamically, so a transcript without a
 * fenced block never pays for the engine and a fenced block renders plain text
 * until its chunk arrives.
 */

import type { HighlighterCore } from 'shiki/core';

/** Grammars the transcript can render. Anything else falls back to plain text. */
const LANGUAGE_MODULES = {
  bash: () => import('shiki/langs/bash.mjs'),
  css: () => import('shiki/langs/css.mjs'),
  diff: () => import('shiki/langs/diff.mjs'),
  go: () => import('shiki/langs/go.mjs'),
  html: () => import('shiki/langs/html.mjs'),
  js: () => import('shiki/langs/js.mjs'),
  json: () => import('shiki/langs/json.mjs'),
  jsx: () => import('shiki/langs/jsx.mjs'),
  markdown: () => import('shiki/langs/markdown.mjs'),
  python: () => import('shiki/langs/python.mjs'),
  rust: () => import('shiki/langs/rust.mjs'),
  shellscript: () => import('shiki/langs/shellscript.mjs'),
  sql: () => import('shiki/langs/sql.mjs'),
  toml: () => import('shiki/langs/toml.mjs'),
  ts: () => import('shiki/langs/ts.mjs'),
  tsx: () => import('shiki/langs/tsx.mjs'),
  yaml: () => import('shiki/langs/yaml.mjs'),
} as const;

type Language = keyof typeof LANGUAGE_MODULES;

const LANGUAGE_SET = new Set<string>(Object.keys(LANGUAGE_MODULES));

/** Map common fence aliases onto a loaded grammar. */
const ALIASES: Record<string, Language | 'text'> = {
  sh: 'bash',
  zsh: 'bash',
  shell: 'bash',
  console: 'bash',
  javascript: 'js',
  node: 'js',
  typescript: 'ts',
  jsonc: 'json',
  yml: 'yaml',
  rs: 'rust',
  py: 'python',
  golang: 'go',
  md: 'markdown',
  text: 'text',
  plaintext: 'text',
  txt: 'text',
  plain: 'text',
};

const LIGHT_THEME = 'rose-pine-dawn';
const DARK_THEME = 'rose-pine-moon';

/** Highlighted results kept; the transcript re-renders often. */
const CACHE_LIMIT = 200;

let highlighterPromise: Promise<HighlighterCore> | null = null;
const cache = new Map<string, string>();

/** Resolve a fence info string to a loaded grammar, or `null` for plain text. */
export function resolveLanguage(info: string | undefined): Language | null {
  const name = (info ?? '').trim().toLowerCase().split(/[\s{]/)[0];
  if (!name) return null;
  const alias = ALIASES[name];
  if (alias === 'text') return null;
  if (alias) return alias;
  return LANGUAGE_SET.has(name) ? (name as Language) : null;
}

/** Load the highlighter once. Rejects if the chunks cannot be fetched. */
export function loadHighlighter(): Promise<HighlighterCore> {
  if (!highlighterPromise) {
    highlighterPromise = Promise.all([import('shiki/core'), import('shiki/engine/javascript')])
      .then(([{ createHighlighterCore }, { createJavaScriptRegexEngine }]) =>
        createHighlighterCore({
          themes: [
            import('shiki/themes/rose-pine-dawn.mjs'),
            import('shiki/themes/rose-pine-moon.mjs'),
          ],
          langs: (Object.keys(LANGUAGE_MODULES) as Language[]).map((language) =>
            LANGUAGE_MODULES[language](),
          ),
          engine: createJavaScriptRegexEngine({ forgiving: true }),
        }),
      )
      .catch((error) => {
        // Allow a later mount to retry after a transient chunk failure.
        highlighterPromise = null;
        throw error;
      });
  }
  return highlighterPromise;
}

function cacheKey(code: string, language: Language, theme: string): string {
  return `${theme}:${language}:${code}`;
}

/**
 * Highlight a code block to HTML.
 *
 * Shiki builds the markup from the code text, never from user HTML. The
 * returned string is injected through `dangerouslySetInnerHTML`, which is safe
 * here because the only producer is Shiki's token renderer.
 */
export async function highlight(
  code: string,
  language: Language,
  theme: 'light' | 'dark',
): Promise<string> {
  const themeName = theme === 'dark' ? DARK_THEME : LIGHT_THEME;
  const key = cacheKey(code, language, themeName);
  const cached = cache.get(key);
  if (cached !== undefined) {
    // Refresh recency.
    cache.delete(key);
    cache.set(key, cached);
    return cached;
  }

  const highlighter = await loadHighlighter();
  const html = highlighter.codeToHtml(code, { lang: language, theme: themeName });
  cache.set(key, html);
  if (cache.size > CACHE_LIMIT) {
    const oldest = cache.keys().next();
    if (!oldest.done) cache.delete(oldest.value);
  }
  return html;
}

/** Drop cached output, for tests. */
export function clearHighlightCache(): void {
  cache.clear();
}
