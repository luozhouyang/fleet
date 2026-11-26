/**
 * Contrast checks for the theme token pairs.
 *
 * The palette lives in `styles.css`; this module reads the two token blocks and
 * verifies the pairs that carry text and UI boundaries in both themes. It runs
 * in the unit suite, so a palette edit that drops text below WCAG AA fails fast.
 */

export interface Rgb {
  r: number;
  g: number;
  b: number;
}

/** Parse `#rgb` or `#rrggbb`. */
export function parseHex(hex: string): Rgb {
  const value = hex.trim().replace(/^#/, '');
  if (value.length === 3) {
    return {
      r: Number.parseInt(value[0] + value[0], 16),
      g: Number.parseInt(value[1] + value[1], 16),
      b: Number.parseInt(value[2] + value[2], 16),
    };
  }
  if (value.length !== 6) throw new Error(`not a hex color: ${hex}`);
  return {
    r: Number.parseInt(value.slice(0, 2), 16),
    g: Number.parseInt(value.slice(2, 4), 16),
    b: Number.parseInt(value.slice(4, 6), 16),
  };
}

function linear(channel: number): number {
  const value = channel / 255;
  return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
}

/** WCAG relative luminance. */
export function luminance(color: Rgb): number {
  return 0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b);
}

/** WCAG contrast ratio, between 1 and 21. */
export function contrastRatio(foreground: string, background: string): number {
  const a = luminance(parseHex(foreground));
  const b = luminance(parseHex(background));
  const lighter = Math.max(a, b);
  const darker = Math.min(a, b);
  return (lighter + 0.05) / (darker + 0.05);
}

/**
 * Extract `--color-*` values from a CSS block.
 *
 * `selector` is matched literally (e.g. `:root[data-theme='dark']`); the block
 * runs from the selector's `{` to the matching `}`.
 */
export function readTokenBlock(css: string, selector: string): Record<string, string> {
  const start = css.indexOf(selector);
  if (start === -1) throw new Error(`missing theme selector: ${selector}`);
  const open = css.indexOf('{', start);
  const close = css.indexOf('\n}', open);
  const body = css.slice(open + 1, close);
  const tokens: Record<string, string> = {};
  for (const match of body.matchAll(/--color-([a-z0-9-]+):\s*(#[0-9a-fA-F]{3,8});/g)) {
    tokens[match[1]] = match[2];
  }
  return tokens;
}

/**
 * Pairs that must meet a ratio: body text on each surface it lands on, faint
 * metadata text, and the status tones used as text and as a dot on its surface.
 */
export const REQUIRED_PAIRS: Array<{ foreground: string; background: string; min: number }> = [
  { foreground: 'body', background: 'background', min: 4.5 },
  { foreground: 'body', background: 'card', min: 4.5 },
  { foreground: 'body', background: 'panel', min: 4.5 },
  { foreground: 'body', background: 'chat-pane', min: 4.5 },
  { foreground: 'foreground', background: 'background', min: 4.5 },
  { foreground: 'foreground', background: 'card', min: 4.5 },
  { foreground: 'muted-foreground', background: 'card', min: 4.5 },
  { foreground: 'muted-foreground', background: 'panel', min: 4.5 },
  { foreground: 'faint', background: 'card', min: 4.5 },
  { foreground: 'faint', background: 'panel', min: 4.5 },
  { foreground: 'primary-foreground', background: 'primary', min: 4.5 },
  { foreground: 'status-failed', background: 'card', min: 4.5 },
  { foreground: 'status-running', background: 'card', min: 4.5 },
  { foreground: 'status-waiting', background: 'card', min: 4.5 },
  { foreground: 'code-fg', background: 'code-bg', min: 4.5 },
  { foreground: 'diff-add-fg', background: 'diff-add-bg', min: 4.5 },
  { foreground: 'diff-del-fg', background: 'diff-del-bg', min: 4.5 },
  { foreground: 'border-strong', background: 'background', min: 1.3 },
];
