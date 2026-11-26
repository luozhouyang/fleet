import { Check, Copy } from 'lucide-react';
import { useEffect, useState } from 'react';
import { highlight, resolveLanguage } from '@/lib/markdown/highlighter';
import { useTheme } from '@/lib/use-theme';
import { cn } from '@/lib/utils/cn';

/**
 * A fenced code block.
 *
 * Highlighting loads lazily and the block renders plain text until the
 * highlighter is ready, so a slow chunk never blocks the transcript. The
 * highlighted markup is produced by Shiki from the code text.
 */
export function CodeBlock({ code, language }: { code: string; language?: string }) {
  const { resolved } = useTheme();
  const grammar = resolveLanguage(language);
  const [html, setHtml] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!grammar) {
      setHtml(null);
      return;
    }
    let cancelled = false;
    void highlight(code, grammar, resolved).then(
      (result) => {
        if (!cancelled) setHtml(result);
      },
      () => {
        // Leave the plain fallback in place.
        if (!cancelled) setHtml(null);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [code, grammar, resolved]);

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1_500);
    return () => window.clearTimeout(timer);
  }, [copied]);

  const copy = () => {
    void navigator.clipboard?.writeText(code).then(
      () => setCopied(true),
      () => undefined,
    );
  };

  return (
    <div className="group/code my-2 overflow-hidden rounded-md border border-code-border bg-code-bg">
      <div className="flex items-center gap-2 border-b border-code-border px-2.5 py-1">
        <span className="font-mono text-[9.5px] uppercase tracking-[0.08em] text-faint">
          {language?.trim() || 'text'}
        </span>
        <button
          type="button"
          onClick={copy}
          aria-label={copied ? 'Copied' : 'Copy code'}
          className="ml-auto flex items-center gap-1 rounded px-1 py-0.5 text-[9.5px] text-faint opacity-0 transition-opacity hover:text-foreground focus-visible:opacity-100 group-hover/code:opacity-100"
        >
          {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>
      {html ? (
        <div
          className={cn(
            'overflow-x-auto p-0 text-[11px] leading-[17px]',
            '[&_pre]:!bg-transparent [&_pre]:!p-2.5 [&_code]:font-mono',
          )}
          // Shiki markup is generated from the code text, never from HTML.
          dangerouslySetInnerHTML={{ __html: html }}
        />
      ) : (
        <pre className="overflow-x-auto p-2.5 font-mono text-[11px] leading-[17px] text-code-fg">
          {code}
        </pre>
      )}
    </div>
  );
}
