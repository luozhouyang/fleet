import type { Monaco } from '@monaco-editor/react';

export const MONACO_THEME = 'agent-fleet-dark';

export function defineAgentFleetTheme(monaco: Monaco): void {
  monaco.editor.defineTheme(MONACO_THEME, {
    base: 'vs-dark',
    inherit: true,
    rules: [
      { token: 'comment', foreground: '64748b' },
      { token: 'keyword', foreground: '5eead4' },
      { token: 'string', foreground: 'fcd34d' },
      { token: 'number', foreground: 'f0abfc' },
      { token: 'type', foreground: '7dd3fc' },
    ],
    colors: {
      'editor.background': '#080f1d',
      'editor.foreground': '#cbd5e1',
      'editorLineNumber.foreground': '#475569',
      'editorLineNumber.activeForeground': '#94a3b8',
      'editor.selectionBackground': '#0f766e66',
      'editor.inactiveSelectionBackground': '#0f766e33',
      'editor.lineHighlightBackground': '#0f172a',
      'editorCursor.foreground': '#2dd4bf',
      'editorIndentGuide.background1': '#1e293b',
      'editorIndentGuide.activeBackground1': '#334155',
      'editorGutter.background': '#080f1d',
      'editorWidget.background': '#0f172a',
      'editorWidget.border': '#334155',
      'diffEditor.insertedTextBackground': '#10b98122',
      'diffEditor.removedTextBackground': '#ef444422',
      'diffEditor.insertedLineBackground': '#10b98118',
      'diffEditor.removedLineBackground': '#ef444418',
    },
  });
}
