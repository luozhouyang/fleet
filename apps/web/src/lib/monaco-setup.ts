// Bundle Monaco with the app instead of loading it from a public CDN at
// runtime. The agent admin may be deployed on an isolated network.
import { loader } from '@monaco-editor/react';
import * as monaco from 'monaco-editor';
import EditorWorker from 'monaco-editor/editor/editor.worker.js?worker';
import JsonWorker from 'monaco-editor/language/json/json.worker.js?worker';
import TypeScriptWorker from 'monaco-editor/language/typescript/ts.worker.js?worker';

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
}
