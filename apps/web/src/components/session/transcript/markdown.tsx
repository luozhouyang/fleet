import { memo, useMemo } from 'react';
import ReactMarkdown, { type Components } from 'react-markdown';
import rehypeSanitize from 'rehype-sanitize';
import remarkGfm from 'remark-gfm';
import { MAX_MARKDOWN_CHARS, markdownSchema } from '@/lib/markdown/sanitize-schema';
import { cn } from '@/lib/utils/cn';
import { CodeBlock } from './code-block';

/**
 * Renders agent markdown.
 *
 * Raw HTML is never enabled; `rehype-sanitize` runs the transcript schema on
 * everything that survives parsing, and the `code` renderer replaces fenced
 * blocks with our own highlighter while inline code stays styled text.
 */
export const Markdown = memo(function Markdown({
  content,
  streaming,
}: {
  content: string;
  streaming: boolean;
}) {
  const bounded =
    content.length > MAX_MARKDOWN_CHARS ? content.slice(0, MAX_MARKDOWN_CHARS) : content;
  const components = useMemo<Components>(
    () => ({
      pre({ children }) {
        // A fenced block arrives as `pre > code`; the code renderer already
        // produced a `CodeBlock`, so the `pre` wrapper is redundant.
        return <>{children}</>;
      },
      code({ className, children, ...props }) {
        const match = /language-([\w+#.-]+)/.exec(className ?? '');
        const text = String(children ?? '');
        if (match) {
          return <CodeBlock code={text.replace(/\n$/, '')} language={match[1]} />;
        }
        return (
          <code
            className={cn(
              'rounded bg-inline-code-bg px-1 py-0.5 font-mono text-[0.92em] text-code-fg',
              className,
            )}
            {...props}
          >
            {children}
          </code>
        );
      },
      a({ children, href, ...props }) {
        return (
          <a
            {...props}
            href={href}
            target="_blank"
            rel="noopener noreferrer"
            className="font-medium text-primary underline decoration-primary/40 underline-offset-2 hover:decoration-primary"
          >
            {children}
          </a>
        );
      },
      table({ children }) {
        return (
          <div className="my-2 overflow-x-auto">
            <table className="w-full border-collapse text-[11.5px]">{children}</table>
          </div>
        );
      },
      th({ children }) {
        return (
          <th className="border border-border bg-muted px-2 py-1 text-left font-bold">
            {children}
          </th>
        );
      },
      td({ children }) {
        return <td className="border border-border px-2 py-1 align-top">{children}</td>;
      },
      blockquote({ children }) {
        return (
          <blockquote className="my-2 border-l-2 border-primary/40 bg-chat-quote py-1 pl-3 text-muted-foreground">
            {children}
          </blockquote>
        );
      },
      ul({ children }) {
        return <ul className="my-1.5 list-disc pl-5">{children}</ul>;
      },
      ol({ children }) {
        return <ol className="my-1.5 list-decimal pl-5">{children}</ol>;
      },
      p({ children }) {
        return <p className="my-1.5 break-words">{children}</p>;
      },
      hr() {
        return <hr className="my-3 border-border" />;
      },
    }),
    [],
  );

  return (
    <div className="transcript-markdown break-words text-[12.5px] leading-[20px] text-body">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[[rehypeSanitize, markdownSchema]]}
        components={components}
        skipHtml
      >
        {bounded}
      </ReactMarkdown>
      {streaming ? (
        <span
          aria-hidden="true"
          className="ml-0.5 inline-block h-[13px] w-[5px] translate-y-[2px] animate-pulse rounded-sm bg-primary motion-reduce:animate-none"
        />
      ) : null}
    </div>
  );
});
