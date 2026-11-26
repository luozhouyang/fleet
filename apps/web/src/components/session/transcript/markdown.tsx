import { memo, useMemo } from 'react';
import ReactMarkdown, { type Components } from 'react-markdown';
import type { Element, ElementContent } from 'hast';
import rehypeSanitize from 'rehype-sanitize';
import remarkGfm from 'remark-gfm';
import { MAX_MARKDOWN_CHARS, markdownSchema } from '@/lib/markdown/sanitize-schema';
import { cn } from '@/lib/utils/cn';
import { CodeBlock } from './code-block';

/** The text of a hast subtree. */
function hastText(node: Element | ElementContent): string {
  if (node.type === 'text') return node.value;
  if (node.type !== 'element') return '';
  return node.children.map(hastText).join('');
}

/**
 * Renders agent markdown.
 *
 * Raw HTML is never enabled; `rehype-sanitize` runs the transcript schema on
 * everything that survives parsing, and the `pre` renderer replaces fenced
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
      pre({ node, children }) {
        // A fenced block arrives as `pre > code`. Render it here so an
        // unlabelled fence keeps its block layout; the `code` renderer then
        // only ever handles inline code.
        const child = node?.children.find(
          (candidate): candidate is Element =>
            candidate.type === 'element' && candidate.tagName === 'code',
        );
        if (child) {
          const className = Array.isArray(child.properties?.className)
            ? child.properties.className.join(' ')
            : String(child.properties?.className ?? '');
          const match = /language-([\w+#.-]+)/.exec(className);
          return <CodeBlock code={hastText(child).replace(/\n$/, '')} language={match?.[1]} />;
        }
        return (
          <pre className="my-2 overflow-x-auto rounded-md bg-code-bg p-2.5 font-mono text-[11px] leading-[17px] text-code-fg">
            {children}
          </pre>
        );
      },
      code({ className, children, ...props }) {
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
      input({ type, checked, ...props }) {
        if (type !== 'checkbox') return <input type={type} {...props} />;
        // GFM task-list checkboxes are display-only; a controlled `checked`
        // without a change handler would warn.
        return (
          <input
            type="checkbox"
            disabled
            defaultChecked={Boolean(checked)}
            className="mr-1.5 translate-y-[1px] accent-primary"
          />
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
