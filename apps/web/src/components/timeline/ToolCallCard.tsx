import type { SessionToolCall } from '@/lib/session-reducer';
import { Badge, cn } from '@agent-fleet/ui';
import { ToolStatusBadge } from '@/components/status/StatusBadge';
import { formatJson, formatTime } from '@/lib/utils/format';

export function ToolCallCard({ tool }: { tool: SessionToolCall }) {
  const input = tool.inputDraft ?? formatJson(tool.input);
  const output = tool.output === undefined ? null : formatJson(tool.output);

  return (
    <details className="group w-full max-w-[52rem] overflow-hidden rounded-lg border border-slate-800 bg-slate-900/45">
      <summary className="flex cursor-pointer list-none items-center gap-2.5 px-3 py-2.5 text-xs text-slate-300 transition-colors hover:bg-slate-900/80 [&::-webkit-details-marker]:hidden">
        <svg
          viewBox="0 0 16 16"
          fill="none"
          className="size-3 shrink-0 text-slate-500 transition-transform group-open:rotate-90"
          aria-hidden="true"
        >
          <path
            d="m6 3.5 4.5 4.5L6 12.5"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
        <span className="font-mono text-[11px] text-slate-500">tool</span>
        <span className="truncate font-medium text-slate-200">{tool.name}</span>
        <ToolStatusBadge status={tool.status} />
        <span className="ml-auto shrink-0 text-[10px] text-slate-600">
          {formatTime(tool.started_at)}
        </span>
      </summary>

      <div className="space-y-3 border-t border-slate-800/80 p-3">
        <div className="flex items-center justify-between gap-3 text-[10px] text-slate-600">
          <span className="truncate font-mono">{tool.id}</span>
          {tool.error ? <span className="text-red-400">Failed</span> : null}
        </div>

        <JsonBlock label="Input" value={input} tone="input" />
        {output !== null ? <JsonBlock label="Output" value={output} tone="output" /> : null}
        {tool.error ? (
          <div className="rounded-md border border-red-500/20 bg-red-500/5 p-2 text-xs leading-5 text-red-300">
            {tool.error}
          </div>
        ) : null}
      </div>
    </details>
  );
}

function JsonBlock({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone: 'input' | 'output';
}) {
  return (
    <div>
      <div className="mb-1 flex items-center gap-2">
        <span className="text-[10px] uppercase tracking-wider text-slate-600">{label}</span>
        <Badge tone={tone === 'input' ? 'slate' : 'teal'} className="px-1.5 py-0">
          {tone === 'input' ? 'sent' : 'received'}
        </Badge>
      </div>
      <pre
        className={cn(
          'max-h-64 overflow-auto rounded-md border p-2.5 font-mono text-[11px] leading-5',
          tone === 'input'
            ? 'border-slate-800 bg-slate-950/60 text-slate-400'
            : 'border-teal-500/10 bg-teal-500/5 text-slate-300',
        )}
      >
        {value}
      </pre>
    </div>
  );
}
