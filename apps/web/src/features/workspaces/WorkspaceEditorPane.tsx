import { Download, File, FileDiff, X } from 'lucide-react';
import { useState } from 'react';
import { CodeEditor } from '@/components/editor/code-editor';
import { DiffEditor } from '@/components/editor/diff-editor';
import { EmptyState, ErrorState, LoadingState } from '@/components/ui/feedback';
import { apiClient } from '@/lib/api/client';
import { useFileContent, useGitDiff } from '@/lib/query/hooks';
import { cn } from '@/lib/utils/cn';
import { saveBlob } from '@/lib/utils/download';
import { formatBytes, getErrorMessage } from '@/lib/utils/format';
import { languageForPath } from '@/lib/utils/language';
import { buildDiffSides, gitStatusClass, gitStatusLetter } from './workspace-utils';

export interface EditorTab {
  id: string;
  kind: 'file' | 'diff';
  path: string;
  name: string;
  staged?: boolean;
}

function DownloadButton({
  label,
  downloading,
  onClick,
}: {
  label: string;
  downloading: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={downloading}
      className="inline-flex shrink-0 items-center gap-1 rounded-sm border border-border bg-card px-1.5 py-0.5 text-[10.5px] font-bold text-body transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-60"
    >
      <Download className="size-3" aria-hidden="true" />
      {downloading ? 'Downloading…' : label}
    </button>
  );
}

function FileTabView({ workspaceId, path }: { workspaceId: string; path: string }) {
  const fileQuery = useFileContent(workspaceId, path);
  const [downloading, setDownloading] = useState(false);
  const [downloadError, setDownloadError] = useState<string | null>(null);

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

  if (fileQuery.isLoading) return <LoadingState label="Loading file…" />;
  if (fileQuery.isError) {
    return (
      <ErrorState
        title="Could not open file"
        message={getErrorMessage(fileQuery.error)}
        onRetry={() => void fileQuery.refetch()}
      />
    );
  }

  const file = fileQuery.data;
  if (!file) {
    return <EmptyState title="Empty file" description="This file has no content to show." />;
  }
  if (file.is_binary) {
    return (
      <div className="flex h-full min-h-0 flex-col">
        {downloadError ? (
          <div className="shrink-0 border-b border-border bg-destructive-soft px-3 py-2 text-[11px] leading-4 text-destructive">
            {downloadError}
          </div>
        ) : null}
        <EmptyState
          title="Binary file"
          description="This file cannot be displayed in the text editor."
          icon={<File className="size-4" aria-hidden="true" />}
          className="flex-1"
          action={
            <DownloadButton
              label="Download file"
              downloading={downloading}
              onClick={() => void handleDownload()}
            />
          }
        />
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      {file.truncated ? (
        <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border bg-warning-soft px-3 py-1.5 text-[11px] text-warning">
          <span className="min-w-0 flex-1">
            On-disk size {formatBytes(file.size)}; the preview was truncated.
          </span>
          <DownloadButton
            label="Full file"
            downloading={downloading}
            onClick={() => void handleDownload()}
          />
        </div>
      ) : null}
      {downloadError ? (
        <div className="shrink-0 border-b border-border bg-destructive-soft px-3 py-2 text-[11px] leading-4 text-destructive">
          {downloadError}
        </div>
      ) : null}
      <div className="min-h-0 flex-1">
        <CodeEditor value={file.content} language={languageForPath(path, file.language)} readOnly />
      </div>
    </div>
  );
}

function DiffTabView({
  workspaceId,
  path,
  staged,
}: {
  workspaceId: string;
  path: string;
  staged: boolean;
}) {
  const diffOptions = staged ? { path, staged: true } : { path, include_untracked: true };
  const diffQuery = useGitDiff(workspaceId, diffOptions);
  const [downloadingRaw, setDownloadingRaw] = useState(false);
  const [rawDownloadError, setRawDownloadError] = useState<string | null>(null);
  const [rawDownloadCapped, setRawDownloadCapped] = useState(false);

  async function handleDownloadRaw() {
    setDownloadingRaw(true);
    setRawDownloadError(null);
    try {
      const result = await apiClient.downloadRawDiff(workspaceId, diffOptions);
      saveBlob(result.blob, result.filename);
      setRawDownloadCapped(result.truncated);
    } catch (error) {
      setRawDownloadError(getErrorMessage(error));
    } finally {
      setDownloadingRaw(false);
    }
  }

  if (diffQuery.isLoading) return <LoadingState label="Loading diff…" />;
  if (diffQuery.isError) {
    return (
      <ErrorState
        title="Could not load diff"
        message={getErrorMessage(diffQuery.error)}
        onRetry={() => void diffQuery.refetch()}
      />
    );
  }

  const file =
    diffQuery.data?.files.find((candidate) => candidate.path === path) ?? diffQuery.data?.files[0];

  if (!file) {
    return (
      <EmptyState
        title="No diff"
        description="This file has no changes against the index."
        icon={<FileDiff className="size-4" aria-hidden="true" />}
      />
    );
  }
  if (file.is_binary) {
    return (
      <EmptyState
        title="Binary diff"
        description="Binary files cannot be shown as text."
        icon={<FileDiff className="size-4" aria-hidden="true" />}
      />
    );
  }

  const { original, modified } = buildDiffSides(file);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border px-3">
        <span className={cn('font-mono text-[10px] font-bold', gitStatusClass(file.status))}>
          {gitStatusLetter(file.status)}
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-body">{file.path}</span>
        <span className="font-mono text-[9px] text-success">+{file.additions}</span>
        <span className="font-mono text-[9px] text-destructive">-{file.deletions}</span>
      </div>
      {file.truncated ? (
        <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border bg-warning-soft px-3 py-2 text-[11px] leading-4 text-warning">
          <span className="min-w-0 flex-1">
            Diff capped at 5,000 lines or 512 KiB.{' '}
            {rawDownloadCapped
              ? 'The raw diff was also capped by the daemon output limit.'
              : 'Download the raw patch for the rest.'}
          </span>
          <DownloadButton
            label="Raw diff"
            downloading={downloadingRaw}
            onClick={() => void handleDownloadRaw()}
          />
        </div>
      ) : null}
      {rawDownloadError ? (
        <div className="shrink-0 border-b border-border bg-destructive-soft px-3 py-2 text-[11px] leading-4 text-destructive">
          {rawDownloadError}
        </div>
      ) : null}
      <div className="min-h-0 flex-1">
        <DiffEditor original={original} modified={modified} language={languageForPath(file.path)} />
      </div>
    </div>
  );
}

export function WorkspaceEditorPane({
  workspaceId,
  tabs,
  activeTabId,
  onSelect,
  onClose,
}: {
  workspaceId: string;
  tabs: EditorTab[];
  activeTabId: string | null;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
}) {
  const activeTab = tabs.find((tab) => tab.id === activeTabId) ?? null;

  return (
    <section className="flex min-h-0 min-w-[360px] flex-1 flex-col overflow-hidden rounded-lg border border-border bg-card shadow-card">
      <div className="flex h-10 shrink-0 items-stretch overflow-x-auto border-b border-border">
        {tabs.length === 0 ? (
          <span className="flex items-center px-3 font-mono text-[9px] uppercase tracking-[0.14em] text-faint">
            NO OPEN FILES
          </span>
        ) : (
          tabs.map((tab) => {
            const active = tab.id === activeTab?.id;
            return (
              <div
                key={tab.id}
                className={cn(
                  'flex shrink-0 items-center border-b-2 px-1 transition-colors',
                  active ? 'border-primary' : 'border-transparent',
                )}
              >
                <button
                  type="button"
                  onClick={() => onSelect(tab.id)}
                  title={tab.path}
                  aria-current={active ? 'page' : undefined}
                  className={cn(
                    'flex h-9 max-w-[220px] items-center gap-1.5 pl-2 text-[11.5px] outline-none focus-visible:ring-2 focus-visible:ring-ring/35',
                    active
                      ? 'font-medium text-foreground'
                      : 'text-muted-foreground hover:text-foreground',
                  )}
                >
                  {tab.kind === 'diff' ? (
                    <FileDiff className="size-3 shrink-0 text-faint" aria-hidden="true" />
                  ) : (
                    <File className="size-3 shrink-0 text-faint" aria-hidden="true" />
                  )}
                  <span className="truncate">{tab.name}</span>
                </button>
                <button
                  type="button"
                  onClick={() => onClose(tab.id)}
                  aria-label={`Close ${tab.name}`}
                  title={`Close ${tab.name}`}
                  className="ml-0.5 flex size-6 shrink-0 items-center justify-center rounded text-faint transition-colors hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/35"
                >
                  <X className="size-3" aria-hidden="true" />
                </button>
              </div>
            );
          })
        )}
      </div>

      <div className="min-h-0 flex-1">
        {activeTab ? (
          activeTab.kind === 'file' ? (
            <FileTabView key={activeTab.id} workspaceId={workspaceId} path={activeTab.path} />
          ) : (
            <DiffTabView
              key={activeTab.id}
              workspaceId={workspaceId}
              path={activeTab.path}
              staged={Boolean(activeTab.staged)}
            />
          )
        ) : (
          <EmptyState
            title="Open a file or a changed file"
            description="Pick a file from the tree or select a change to view its diff."
            icon={<FileDiff className="size-4" aria-hidden="true" />}
          />
        )}
      </div>
    </section>
  );
}
