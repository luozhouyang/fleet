/**
 * Markdown sanitization policy.
 *
 * Agent output is untrusted. The renderer never enables raw HTML, and this
 * schema is the second line of defense: it narrows the default GitHub-style
 * schema to what a transcript actually needs, removes image rendering
 * (attachments go through the attachment component), and restricts links to
 * protocols a browser cannot turn into script execution.
 */

import { defaultSchema, type Options as SanitizeOptions } from 'rehype-sanitize';

const ALLOWED_TAGS = [
  'a',
  'b',
  'blockquote',
  'br',
  'code',
  'dd',
  'del',
  'details',
  'div',
  'dl',
  'dt',
  'em',
  'h1',
  'h2',
  'h3',
  'h4',
  'h5',
  'h6',
  'hr',
  'i',
  'input',
  'ins',
  'kbd',
  'li',
  'ol',
  'p',
  'pre',
  'q',
  's',
  'samp',
  'section',
  'span',
  'strong',
  'sub',
  'summary',
  'sup',
  'table',
  'tbody',
  'td',
  'tfoot',
  'th',
  'thead',
  'tr',
  'ul',
];

/**
 * The schema handed to `rehype-sanitize`.
 *
 * Compared with the default schema:
 * - `img` and `picture` are dropped, so no remote loads or `onerror` payloads;
 * - `class` survives only on `code`/`pre`/`span` as a `language-*` value,
 *   which the code renderer uses to pick a grammar;
 * - `style` is never allowed;
 * - links keep `http`, `https` and `mailto` only.
 */
export const markdownSchema: SanitizeOptions = {
  ...defaultSchema,
  tagNames: ALLOWED_TAGS,
  attributes: {
    a: ['href', 'title', ['className', 'data-footnote-backref']],
    blockquote: ['cite'],
    code: [['className', /^language-[\w+#.-]+$/]],
    div: ['itemScope', 'itemType'],
    h2: [['className', 'sr-only']],
    input: [
      ['disabled', true],
      ['type', 'checkbox'],
    ],
    li: [['className', 'task-list-item']],
    ol: [['className', 'contains-task-list']],
    pre: [['className', /^language-[\w+#.-]+$/]],
    span: [['className', /^language-[\w+#.-]+$/]],
    section: ['dataFootnotes', ['className', 'footnotes']],
    summary: ['ariaLabel'],
    table: ['ariaLabel'],
    ul: [['className', 'contains-task-list']],
  },
  protocols: {
    cite: ['http', 'https'],
    href: ['http', 'https', 'mailto'],
    src: ['http', 'https'],
  },
  // A bare URL is rendered through `a`; `longDesc` on images cannot occur
  // because `img` is not an allowed tag at all.
  clobberPrefix: 'fleet-md-',
  strip: ['script', 'style', 'iframe', 'object', 'embed', 'form'],
};

/** Maximum characters of markdown rendered per message. */
export const MAX_MARKDOWN_CHARS = 120_000;
