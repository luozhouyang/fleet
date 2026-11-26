// Bundle Monaco with the app instead of loading it from a public CDN at
// runtime. The agent admin may be deployed on an isolated network.
import { loader } from '@monaco-editor/react';
import * as monaco from 'monaco-editor';
import EditorWorker from 'monaco-editor/editor/editor.worker.js?worker';
import JsonWorker from 'monaco-editor/language/json/json.worker.js?worker';
import TypeScriptWorker from 'monaco-editor/language/typescript/ts.worker.js?worker';

import type { ThemeMode } from './theme';

self.MonacoEnvironment = {
  getWorker(_workerId: string, label: string) {
    if (label === 'json') return new JsonWorker();
    if (label === 'typescript' || label === 'javascript') return new TypeScriptWorker();
    return new EditorWorker();
  },
};

loader.config({ monaco });

import type { Monaco } from '@monaco-editor/react';

export const MONACO_THEME = 'daylight-bench';
export const MONACO_DARK_THEME = 'ink-bench';

export function defineAgentFleetTheme(instance: Monaco): void {
  instance.editor.defineTheme(MONACO_THEME, {
    base: 'vs',
    inherit: true,
    rules: [
      { token: 'comment', foreground: '97927f' },
      { token: 'keyword', foreground: 'b64a25' },
      { token: 'keyword.control', foreground: 'b64a25' },
      { token: 'string', foreground: '2e6b4c' },
      { token: 'number', foreground: '8e620a' },
      { token: 'type', foreground: '6b4e8f' },
      { token: 'function', foreground: '9e3f1e' },
      { token: 'constant', foreground: '8e620a' },
      { token: 'operator', foreground: '444135' },
    ],
    colors: {
      'editor.background': '#fffdf8',
      'editor.foreground': '#444135',
      'editorLineNumber.foreground': '#97927f',
      'editorLineNumber.activeForeground': '#6b6657',
      'editor.selectionBackground': '#b64a2533',
      'editor.inactiveSelectionBackground': '#b64a251a',
      'editor.lineHighlightBackground': '#f1ede2',
      'editorCursor.foreground': '#b64a25',
      'editorIndentGuide.background1': '#ece8dc',
      'editorIndentGuide.activeBackground1': '#d5cebc',
      'editorGutter.background': '#fffdf8',
      'editorWidget.background': '#fffdf8',
      'editorWidget.border': '#e4dfd1',
      'editorSuggestWidget.background': '#fffdf8',
      'editorSuggestWidget.border': '#e4dfd1',
      'diffEditor.insertedTextBackground': '#2e6b4c22',
      'diffEditor.removedTextBackground': '#a32c2222',
      'diffEditor.insertedLineBackground': '#e4efe788',
      'diffEditor.removedLineBackground': '#f8e6e288',
    },
  });

  // Mirrors the dark palette in `styles.css`. Monaco cannot resolve CSS
  // variables, so the values live here as well; keep the two in step.
  instance.editor.defineTheme(MONACO_DARK_THEME, {
    base: 'vs-dark',
    inherit: true,
    rules: [
      { token: 'comment', foreground: '8f897a' },
      { token: 'keyword', foreground: 'e0703f' },
      { token: 'keyword.control', foreground: 'e0703f' },
      { token: 'string', foreground: '6fbf94' },
      { token: 'number', foreground: 'd9a63c' },
      { token: 'type', foreground: 'b9a3e0' },
      { token: 'function', foreground: 'ef8455' },
      { token: 'constant', foreground: 'd9a63c' },
      { token: 'operator', foreground: 'c8c2b2' },
    ],
    colors: {
      'editor.background': '#1b1811',
      'editor.foreground': '#c8c2b2',
      'editorLineNumber.foreground': '#6f6a5c',
      'editorLineNumber.activeForeground': '#a8a291',
      'editor.selectionBackground': '#e0703f40',
      'editor.inactiveSelectionBackground': '#e0703f22',
      'editor.lineHighlightBackground': '#242118',
      'editorCursor.foreground': '#e0703f',
      'editorIndentGuide.background1': '#2e2a20',
      'editorIndentGuide.activeBackground1': '#453f31',
      'editorGutter.background': '#1b1811',
      'editorWidget.background': '#201d16',
      'editorWidget.border': '#322d22',
      'editorSuggestWidget.background': '#201d16',
      'editorSuggestWidget.border': '#322d22',
      'diffEditor.insertedTextBackground': '#6fbf9422',
      'diffEditor.removedTextBackground': '#e4796b22',
      'diffEditor.insertedLineBackground': '#1b2b2288',
      'diffEditor.removedLineBackground': '#2f1d1a88',
    },
  });
}

/** The Monaco theme id for a resolved app palette. */
export function monacoThemeFor(resolved: 'light' | 'dark'): string {
  return resolved === 'dark' ? MONACO_DARK_THEME : MONACO_THEME;
}

/** Map the app's theme mode onto `@monaco-editor/react`'s `theme` prop. */
export function monacoThemeForMode(mode: ThemeMode): string {
  return mode === 'dark' ? MONACO_DARK_THEME : MONACO_THEME;
}
