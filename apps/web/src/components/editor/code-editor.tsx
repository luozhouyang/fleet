import Editor, { type BeforeMount, type OnChange } from '@monaco-editor/react';
import type { editor } from 'monaco-editor';
import { defineAgentFleetTheme, monacoThemeFor } from '@/lib/monaco-setup';
import { useTheme } from '@/lib/use-theme';
import { cn } from '@/lib/utils/cn';

export function CodeEditor({
  value,
  language = 'plaintext',
  readOnly = true,
  onChange,
  className,
  options,
}: {
  value: string;
  language?: string;
  readOnly?: boolean;
  onChange?: (value: string) => void;
  className?: string;
  options?: editor.IStandaloneEditorConstructionOptions;
}) {
  const { resolved } = useTheme();

  const handleChange: OnChange = (next) => {
    if (onChange && next !== undefined) onChange(next);
  };

  return (
    <div className={cn('h-full min-h-0 w-full', className)}>
      <Editor
        height="100%"
        value={value}
        language={language}
        theme={monacoThemeFor(resolved)}
        beforeMount={defineAgentFleetTheme as BeforeMount}
        onChange={handleChange}
        options={{
          readOnly,
          domReadOnly: readOnly,
          minimap: { enabled: false },
          fontFamily: "'Noto Sans Mono', ui-monospace, monospace",
          fontSize: 12,
          lineHeight: 19,
          scrollBeyondLastLine: false,
          renderLineHighlight: 'none',
          padding: { top: 12, bottom: 12 },
          smoothScrolling: true,
          automaticLayout: true,
          scrollbar: { verticalScrollbarSize: 10, horizontalScrollbarSize: 10 },
          ...options,
        }}
      />
    </div>
  );
}
