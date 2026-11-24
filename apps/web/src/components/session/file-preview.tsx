import { Download, Folder } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { ErrorState, LoadingState } from '@/components/ui/feedback';
import { apiClient } from '@/lib/api/client';
import { useFileContent } from '@/lib/query/hooks';
import { saveBlob } from '@/lib/utils/download';
import { formatBytes, getErrorMessage } from '@/lib/utils/format';

const MAX_PREVIEW_LINES = 4000;

export function FilePreview({ workspaceId, path }: { workspaceId: string; path: string }) {
  const query = useFileContent(workspaceId, path);
  const [downloading, setDownloading] = useState(false);
  const [downloadError, setDownloadError] = useState<string | null>(null);

  useEffect(() => {
    setDownloadError(null);
  }, [path]);

  async function handleDownload() {
    setDownloading(true);
    setDownloadError(null);
    try {
      const result = await apiClient.downloadFile(workspaceId, path);
      saveBlob(result.blob, result.filename);
    } catch (error) {
      setDownloadError(getErrorMessage(error));
    } finally {
      setDownloading(false);
    }
  }

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
      downloading={downloading}
      downloadError={downloadError}
      onDownload={() => void handleDownload()}
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
  downloading,
  downloadError,
  onDownload,
}: {
  path: string;
  content: string;
  size: number;
  truncated: boolean;
  isBinary: boolean;
  encoding: string;
  downloading: boolean;
  downloadError: string | null;
  onDownload: () => void;
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
        <button
          type="button"
          onClick={onDownload}
          disabled={downloading}
          title="Download the full file"
          className="ml-auto inline-flex shrink-0 items-center gap-1 rounded-sm px-1.5 py-0.5 font-mono text-[9px] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:cursor-not-allowed disabled:opacity-60"
        >
          <Download className="size-3" aria-hidden="true" />
          {downloading ? 'Downloading…' : 'Download'}
        </button>
        <span className="shrink-0 font-mono text-[9px] uppercase tracking-[0.08em] text-faint">
          read-only
        </span>
      </div>

      {downloadError ? (
        <div className="shrink-0 border-b border-border bg-destructive-soft px-3 py-2 text-[11px] leading-4 text-destructive">
          {downloadError}
        </div>
      ) : null}

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
