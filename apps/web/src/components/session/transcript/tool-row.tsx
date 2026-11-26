import type { AgentToolCall } from '@agent-fleet/protocol-types';
import {
  ChevronRight,
  FilePenLine,
  FilePlus,
  FileText,
  FolderTree,
  Globe,
  ListTodo,
  Loader,
  Search,
  Terminal,
  Wrench,
  type LucideIcon,
} from 'lucide-react';
import { memo, useState } from 'react';
import { StatusDot } from '@/components/ui/status-dot';
import { useSessionStore } from '@/lib/session-store';
import type { SessionToolCall } from '@/lib/session-reducer';
import { toolIsLive } from '@/lib/session/transcript-model';
import {
  diffFromTool,
  kindShowsDiff,
  outputText,
  summarizeTool,
  toolFailed,
  type ToolKind,
} from '@/lib/session/tool-view';
import { cn } from '@/lib/utils/cn';
import { formatDuration } from '@/lib/utils/format';
import { prettyJson, toolStatusMeta } from '../session-utils';
import { DiffBlock } from './diff-block';

const ICONS: Record<ToolKind, LucideIcon> = {
  bash: Terminal,
  edit: FilePenLine,
  write: FilePlus,
  read: FileText,
  search: Search,
  list: FolderTree,
  web: Globe,
  task: ListTodo,
  other: Wrench,
};

function toolDuration(tool: SessionToolCall): string | null {
  if (!tool.completed_at) return null;
  const started = Date.parse(tool.started_at);
  const completed = Date.parse(tool.completed_at);
  if (Number.isNaN(started) || Number.isNaN(completed) || completed < started) return null;
  return formatDuration((completed - started) / 1000);
}

/**
 * One tool call as a single line with an expandable payload.
 *
 * Failures expand automatically so the error is visible without a click. The
 * card keeps `data-testid="tool-card"` and shows the raw tool name, which the
 * existing end-to-end suite relies on.
 */
export const ToolRow = memo(function ToolRow({
  sessionId,
  toolId,
}: {
  sessionId: string;
  toolId: string;
}) {
  const tool = useSessionStore((state) => state.sessions[sessionId]?.tools[toolId]);
  const [expanded, setExpanded] = useState(false);
  if (!tool) return null;

  const summary = summarizeTool(tool);
  const Icon = ICONS[summary.kind];
  const status = toolStatusMeta(tool.status);
  const failed = toolFailed(tool);
  const live = toolIsLive(tool);
  const open = expanded || failed;
  const duration = toolDuration(tool);
  const draft = tool.inputDraft?.trim();

  return (
    <div data-testid="tool-card" data-tool-name={tool.name} className="pb-4">
      <div className="overflow-hidden rounded-lg border border-border bg-card shadow-card-sm">
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setExpanded((value) => !value)}
          className="flex w-full items-center gap-2 px-2.5 py-2 text-left transition-colors hover:bg-muted/60"
        >
          <Icon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
          <span className="shrink-0 font-mono text-[10.5px] font-bold text-foreground">
            {tool.name}
          </span>
          <span className="min-w-0 flex-1 truncate font-mono text-[10.5px] text-muted-foreground">
            {live && draft
              ? draft
              : `${summary.verb} ${summary.target}${summary.detail ? ` in ${summary.detail}` : ''}`}
          </span>
          {live ? (
            <Loader
              className="size-3 shrink-0 animate-spin text-status-running motion-reduce:animate-none"
              aria-label={status.label}
            />
          ) : (
            <StatusDot tone={status.tone} size="sm" label={failed ? status.label : undefined} />
          )}
          {duration ? (
            <span className="shrink-0 font-mono text-[9px] text-faint">{duration}</span>
          ) : null}
          <ChevronRight
            className={cn('size-3.5 shrink-0 text-faint transition-transform', open && 'rotate-90')}
            aria-hidden="true"
          />
        </button>

        {open ? (
          <div className="flex flex-col gap-2 border-t border-border px-2.5 py-2">
            <ToolBody tool={tool} />
            {tool.error ? (
              <p className="whitespace-pre-wrap break-words rounded-md bg-destructive-soft px-2 py-1.5 text-[10.5px] leading-4 text-destructive">
                {tool.error}
              </p>
            ) : null}
          </div>
        ) : null}
      </div>
    </div>
  );
});

function ToolBody({ tool }: { tool: AgentToolCall }) {
  const diff = kindShowsDiff(summarizeTool(tool).kind) ? diffFromTool(tool) : null;
  if (diff) return <DiffBlock diff={diff} />;

  const input = tool.input == null ? '' : prettyJson(tool.input, 8_000);
  const output = outputText(tool.output, 8_000);
  return (
    <>
      {input ? <ToolPre label="Input" text={input} /> : null}
      {output ? <ToolPre label="Output" text={output} /> : null}
      {!input && !output ? <p className="text-[10.5px] text-faint">No payload recorded.</p> : null}
    </>
  );
}

function ToolPre({ label, text }: { label: string; text: string }) {
  return (
    <div>
      <p className="mb-1 font-mono text-[9px] uppercase tracking-[0.12em] text-faint">{label}</p>
      <pre className="max-h-56 overflow-auto whitespace-pre-wrap break-all rounded-md bg-code-bg px-2 py-1.5 font-mono text-[10px] leading-[15px] text-code-fg">
        {text}
      </pre>
    </div>
  );
}
