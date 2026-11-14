import '@/lib/monaco-setup';
import { DiffEditor } from '@monaco-editor/react';
import { cn, Spinner } from '@agent-fleet/ui';
import { MONACO_THEME, defineAgentFleetTheme } from './monaco-theme';

export function FileDiffEditor({
  original,
  modified,
  language,
  className,
}: {
  original: string;
  modified: string;
  language: string;
  className?: string;
}) {
  return (
    <div className={cn('h-full min-h-0 w-full overflow-hidden bg-[#080f1d]', className)}>
      <DiffEditor
        original={original}
        modified={modified}
        language={language}
        theme={MONACO_THEME}
        height="100%"
        beforeMount={defineAgentFleetTheme}
        loading={
          <div className="flex h-full items-center justify-center gap-3 text-xs text-slate-500">
            <Spinner size="sm" label="Loading diff" />
            Loading diff…
          </div>
        }
        options={{
          readOnly: true,
          originalEditable: false,
          automaticLayout: true,
          renderSideBySide: true,
          minimap: { enabled: false },
          fontSize: 13,
          fontFamily: "'JetBrains Mono', 'SFMono-Regular', Menlo, monospace",
          lineHeight: 21,
          scrollBeyondLastLine: false,
          renderWhitespace: 'selection',
          renderLineHighlight: 'none',
          contextmenu: false,
          overviewRulerLanes: 0,
          hideCursorInOverviewRuler: true,
          padding: { top: 12, bottom: 12 },
          scrollbar: { verticalScrollbarSize: 10, horizontalScrollbarSize: 10 },
        }}
      />
    </div>
  );
}
