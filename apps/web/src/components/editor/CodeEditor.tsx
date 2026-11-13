import Editor from '@monaco-editor/react';
import { cn, Spinner } from '@agent-fleet/ui';
import { MONACO_THEME, defineAgentFleetTheme } from './monaco-theme';

export function CodeEditor({
  path,
  value,
  language,
  className,
}: {
  path: string;
  value: string;
  language: string;
  className?: string;
}) {
  return (
    <div className={cn('h-full min-h-0 w-full overflow-hidden bg-[#080f1d]', className)}>
      <Editor
        path={path}
        value={value}
        language={language}
        theme={MONACO_THEME}
        height="100%"
        beforeMount={defineAgentFleetTheme}
        loading={
          <div className="flex h-full items-center justify-center gap-3 text-xs text-slate-500">
            <Spinner size="sm" label="Loading editor" />
            Loading editor…
          </div>
        }
        options={{
          readOnly: true,
          domReadOnly: true,
          automaticLayout: true,
          minimap: { enabled: false },
          fontSize: 13,
          fontFamily: "'JetBrains Mono', 'SFMono-Regular', Menlo, monospace",
          lineHeight: 21,
          lineNumbersMinChars: 3,
          scrollBeyondLastLine: false,
          wordWrap: 'off',
          renderWhitespace: 'selection',
          renderLineHighlight: 'none',
          contextmenu: false,
          overviewRulerLanes: 0,
          hideCursorInOverviewRuler: true,
          padding: { top: 12, bottom: 12 },
          stickyScroll: { enabled: true, maxLineCount: 4 },
          scrollbar: { verticalScrollbarSize: 10, horizontalScrollbarSize: 10 },
        }}
      />
    </div>
  );
}
