import { Folder } from 'lucide-react';
import { useMemo } from 'react';
import { ErrorState, LoadingState } from '@/components/ui/feedback';
import { useFileContent } from '@/lib/query/hooks';
import { formatBytes, getErrorMessage } from '@/lib/utils/format';

const MAX_PREVIEW_LINES = 4000;

export function FilePreview({ workspaceId, path }: { workspaceId: string; path: string }) {
  const query = useFileContent(workspaceId, path);

  if (!path) {
    return (
      <div className="flex flex-1 items-center justify-center px-6 py-10 text-center text-[11.5px] text-muted-foreground">
        Select a file to preview its contents.
      </div>
    );
  }

  if (query.isLoading) {
    return <LoadingState label="Loading file…" className="flex-1" />;
  }

  if (query.isError) {
    return (
      <ErrorState
        title="Could not read file"
        message={getErrorMessage(query.error)}
        onRetry={() => void query.refetch()}
        className="flex-1"
      />
    );
  }

  const file = query.data;
  if (!file) {
    return <LoadingState label="Loading file…" className="flex-1" />;
  }

  return (
    <FileBody
      path={path}
      content={file.content}
      size={file.size}
      truncated={file.truncated}
      isBinary={file.is_binary}
      encoding={file.encoding}
    />
  );
}

function FileBody({
  path,
  content,
  size,
  truncated,
  isBinary,
  encoding,
}: {
  path: string;
  content: string;
  size: number;
  truncated: boolean;
  isBinary: boolean;
  encoding: string;
}) {
  const lines = useMemo(() => content.split('\n'), [content]);
  const visibleLines = useMemo(
    () => (lines.length > MAX_PREVIEW_LINES ? lines.slice(0, MAX_PREVIEW_LINES) : lines),
    [lines],
  );
  const limited = lines.length > MAX_PREVIEW_LINES;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-1.5 border-b border-border px-3 py-1.5">
        <Folder className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span className="truncate font-mono text-[10px] text-body" title={path}>
          {path}
        </span>
        <span className="ml-auto shrink-0 font-mono text-[9px] uppercase tracking-[0.08em] text-faint">
          read-only
        </span>
      </div>

      {isBinary ? (
        <div className="shrink-0 border-b border-border bg-muted px-3 py-2 text-[11px] text-muted-foreground">
          Binary file — preview is unavailable.
        </div>
      ) : truncated || limited ? (
        <div className="shrink-0 border-b border-border bg-warning-soft px-3 py-2 text-[11px] leading-4 text-warning">
          {limited
            ? `Preview limited to ${MAX_PREVIEW_LINES.toLocaleString()} lines.`
            : `Preview truncated at ${formatBytes(size)}.`}
        </div>
      ) : null}

      {isBinary ? (
        <div className="flex flex-1 items-center justify-center px-6 py-10 text-center text-[11.5px] text-muted-foreground">
          This file is not suitable for a text preview.
        </div>
      ) : (
        <div className="min-h-0 flex-1 overflow-auto bg-inset font-mono text-[11px] leading-[18px]">
          <div className="min-w-max py-2">
            {visibleLines.map((line, index) => (
              <div key={index} className="flex">
                <span className="w-10 shrink-0 select-none pr-2 text-right text-faint">
                  {index + 1}
                </span>
                <span className="whitespace-pre pr-4 text-body">{line || ' '}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="flex shrink-0 items-center justify-between gap-2 border-t border-border px-3 py-1.5 font-mono text-[9px] text-faint">
        <span>{formatBytes(size)}</span>
        <span>{encoding}</span>
      </div>
    </div>
  );
}
