import { describe, expect, it } from 'vitest';
import { resolveLanguage } from './highlighter';

describe('language resolution', () => {
  it('accepts loaded grammars', () => {
    expect(resolveLanguage('rust')).toBe('rust');
    expect(resolveLanguage('ts')).toBe('ts');
    expect(resolveLanguage('json')).toBe('json');
  });

  it('maps common aliases', () => {
    expect(resolveLanguage('sh')).toBe('bash');
    expect(resolveLanguage('javascript')).toBe('js');
    expect(resolveLanguage('typescript')).toBe('ts');
    expect(resolveLanguage('yml')).toBe('yaml');
    expect(resolveLanguage('rs')).toBe('rust');
  });

  it('ignores extra fence metadata', () => {
    expect(resolveLanguage('ts title="a.ts"')).toBe('ts');
    expect(resolveLanguage('  Rust  ')).toBe('rust');
  });

  it('returns null for unknown or plain languages', () => {
    expect(resolveLanguage('brainfuck')).toBeNull();
    expect(resolveLanguage('text')).toBeNull();
    expect(resolveLanguage(undefined)).toBeNull();
    expect(resolveLanguage('')).toBeNull();
  });
});
