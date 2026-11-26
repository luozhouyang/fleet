import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { REQUIRED_PAIRS, contrastRatio, parseHex, readTokenBlock } from './theme-contrast';

const css = readFileSync(fileURLToPath(new URL('../styles.css', import.meta.url)), 'utf8');

function assertPairs(tokens: Record<string, string>, label: string) {
  const failures: string[] = [];
  for (const pair of REQUIRED_PAIRS) {
    const foreground = tokens[pair.foreground];
    const background = tokens[pair.background];
    if (!foreground || !background) {
      failures.push(`${pair.foreground} on ${pair.background}: token missing`);
      continue;
    }
    const ratio = contrastRatio(foreground, background);
    if (ratio < pair.min) {
      failures.push(`${pair.foreground} on ${pair.background}: ${ratio.toFixed(2)} < ${pair.min}`);
    }
  }
  expect(failures, `${label} theme contrast:\n${failures.join('\n')}`).toEqual([]);
}

describe('theme contrast', () => {
  it('parses hex colors', () => {
    expect(parseHex('#fff')).toEqual({ r: 255, g: 255, b: 255 });
    expect(parseHex('#b64a25')).toEqual({ r: 182, g: 74, b: 37 });
    expect(() => parseHex('#12345')).toThrow();
  });

  it('computes WCAG ratios', () => {
    expect(contrastRatio('#000000', '#ffffff')).toBeCloseTo(21, 0);
    expect(contrastRatio('#ffffff', '#ffffff')).toBeCloseTo(1, 5);
  });

  it('keeps light-theme text readable', () => {
    assertPairs(readTokenBlock(css, '@theme'), 'light');
  });

  it('keeps dark-theme text readable', () => {
    const light = readTokenBlock(css, '@theme');
    const dark = { ...light, ...readTokenBlock(css, ":root[data-theme='dark']") };
    assertPairs(dark, 'dark');
  });

  it('defines every dark override that the light block defines', () => {
    const light = readTokenBlock(css, '@theme');
    const dark = readTokenBlock(css, ":root[data-theme='dark']");
    const missing = Object.keys(light).filter((token) => !(token in dark));
    expect(missing, `tokens missing a dark value: ${missing.join(', ')}`).toEqual([]);
  });
});
