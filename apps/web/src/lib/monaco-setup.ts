// Bundle Monaco with the app instead of loading it from a public CDN at
// runtime. The control plane may be deployed on an isolated network, and
// relying on jsDelivr made the editor hang on "Loading editor…" whenever the
// CDN was slow or unreachable.
import { loader } from '@monaco-editor/react';
import * as monaco from 'monaco-editor';
import EditorWorker from 'monaco-editor/editor/editor.worker.js?worker';

self.MonacoEnvironment = {
  getWorker: () => new EditorWorker(),
};

loader.config({ monaco });
