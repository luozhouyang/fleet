/**
 * Sanitization fixtures.
 *
 * These run the real unified pipeline (markdown -> HAST -> sanitize -> HTML)
 * with the same schema the transcript uses, so a regression that re-enables a
 * dangerous tag or protocol fails here rather than in a browser.
 */

import { unified } from 'unified';
import rehypeSanitize from 'rehype-sanitize';
import rehypeStringify from 'rehype-stringify';
import remarkGfm from 'remark-gfm';
import remarkParse from 'remark-parse';
import remarkRehype from 'remark-rehype';
import { describe, expect, it } from 'vitest';
import { markdownSchema } from './sanitize-schema';

async function render(markdown: string): Promise<string> {
  const file = await unified()
    .use(remarkParse)
    .use(remarkGfm)
    .use(remarkRehype)
    .use(rehypeSanitize, markdownSchema)
    .use(rehypeStringify)
    .process(markdown);
  return String(file);
}

describe('markdown sanitization', () => {
  it('drops raw html script and event handlers', async () => {
    const html = await render(
      '<script>alert(1)</script>\n\n<img src=x onerror="alert(1)">\n\n<div onclick="steal()">x</div>',
    );
    expect(html).not.toContain('<script');
    expect(html).not.toContain('onerror');
    expect(html).not.toContain('onclick');
    expect(html).not.toContain('<img');
  });

  it('strips javascript and data protocols from links', async () => {
    const html = await render(
      '[a](javascript:alert(1)) [b](data:text/html;base64,PHNjcmlwdD4=) [c](vbscript:msgbox(1))',
    );
    expect(html).not.toContain('javascript:');
    expect(html).not.toContain('data:text/html');
    expect(html).not.toContain('vbscript:');
  });

  it('keeps http, https and mailto links', async () => {
    const html = await render('[ok](https://example.com/x) [mail](mailto:a@example.com)');
    expect(html).toContain('href="https://example.com/x"');
    expect(html).toContain('href="mailto:a@example.com"');
  });

  it('allows fenced code classes and drops arbitrary ones', async () => {
    const html = await render('```rust\nfn main() {}\n```\n\n<span class="evil">x</span>');
    expect(html).toContain('language-rust');
    expect(html).not.toContain('class="evil"');
  });

  it('keeps gfm tables and task lists', async () => {
    const html = await render('| a | b |\n| - | - |\n| 1 | 2 |\n\n- [x] done\n- [ ] todo\n');
    expect(html).toContain('<table>');
    expect(html).toContain('type="checkbox"');
  });

  it('preserves task-list checked state', async () => {
    const html = await render('- [x] done\n- [ ] todo\n');
    // The schema must keep `checked` on inputs; without it every box renders
    // unchecked and a completed list looks unfinished.
    expect(html).toMatch(/checked/);
    expect(html).toContain('disabled');
  });

  it('neutralizes html embedded in a code fence', async () => {
    const html = await render('```html\n<script>alert(1)</script>\n```');
    // Escaped as text, not as a live tag: the stringifier may use named or
    // numeric entities, but no `<script` element may survive.
    expect(html).toContain('script>alert(1)');
    expect(html).not.toContain('<script');
  });

  it('does not allow style tags or attributes', async () => {
    const html = await render('<style>body{display:none}</style>\n\n<p style="color:red">x</p>');
    expect(html).not.toContain('<style');
    expect(html).not.toContain('style="');
  });
});
