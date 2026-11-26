import { DiffEditor as MonacoDiffEditor, type BeforeMount } from '@monaco-editor/react';
import type { editor } from 'monaco-editor';
import { defineAgentFleetTheme, monacoThemeFor } from '@/lib/monaco-setup';
import { useTheme } from '@/lib/use-theme';
import { cn } from '@/lib/utils/cn';

export function DiffEditor({
  original,
  modified,
  language = 'plaintext',
  className,
  options,
}: {
  original: string;
  modified: string;
  language?: string;
  className?: string;
  options?: editor.IStandaloneDiffEditorConstructionOptions;
}) {
  const { resolved } = useTheme();

  return (
    <div className={cn('h-full min-h-0 w-full', className)}>
      <MonacoDiffEditor
        height="100%"
        original={original}
        modified={modified}
        language={language}
        theme={monacoThemeFor(resolved)}
        beforeMount={defineAgentFleetTheme as BeforeMount}
        options={{
          readOnly: true,
          renderSideBySide: false,
          minimap: { enabled: false },
          fontFamily: "'Noto Sans Mono', ui-monospace, monospace",
          fontSize: 12,
          lineHeight: 19,
          scrollBeyondLastLine: false,
          renderLineHighlight: 'none',
          padding: { top: 12, bottom: 12 },
          automaticLayout: true,
          scrollbar: { verticalScrollbarSize: 10, horizontalScrollbarSize: 10 },
          ...options,
        }}
      />
    </div>
  );
}
