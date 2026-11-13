import type {
  FileContent,
  NotModified,
  WorkspaceEvent,
  WorkspaceEventType,
} from '@agent-fleet/protocol-types';
import { Button, EmptyState, Spinner } from '@agent-fleet/ui';
import { useQuery } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { ErrorState } from '@/components/feedback/ErrorState';
import { apiClient, eventClient } from '@/lib/api/client';
import { queryKeys } from '@/lib/query/keys';
import { formatBytes, getErrorMessage } from '@/lib/utils/format';
import { languageForPath } from '@/lib/utils/language';

type DiskChange = {
  title: string;
  detail: string;
};

const changeMessages: Partial<Record<WorkspaceEventType, DiskChange>> = {
  'workspace.file.modified': {
    title: 'File changed on disk',
    detail: 'The version in the editor is stale. Reload explicitly to fetch the new content.',
  },
  'workspace.file.created': {
    title: 'File recreated on disk',
    detail: 'The version in the editor may be stale.',
  },
  'workspace.file.deleted': {
    title: 'File deleted on disk',
    detail: 'The last loaded content is still shown and has not been overwritten.',
  },
  'workspace.file.renamed': {
    title: 'File renamed on disk',
    detail: 'This path is stale. The editor still shows the last loaded content.',
  },
};

export function FileViewer({
  workspaceId,
  path,
}: {
  workspaceId: string;
  path: string;
}) {
  const [refreshToken, setRefreshToken] = useState(0);
  const [cache, setCache] = useState<{ path: string; file: FileContent } | null>(null);
  const [diskChange, setDiskChange] = useState<DiskChange | null>(null);
  const etagsByPath = useRef<Record<string, string>>({});

  const query = useQuery<FileContent | NotModified>({
    queryKey: [...queryKeys.file(workspaceId, path), refreshToken],
    queryFn: ({ signal }) =>
      apiClient.readFile(workspaceId, path, {
        etag: etagsByPath.current[path],
        signal,
      }),
    enabled: Boolean(workspaceId && path),
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: 5 * 60_000,
    retry: 1,
  });

  useEffect(() => {
    const data = query.data;
    if (!data || data.path !== path) return;

    if ('content' in data) {
      etagsByPath.current[path] = data.etag;
      setCache({ path, file: data });
      return;
    }

    etagsByPath.current[path] = data.etag;
    setCache((previous) =>
      previous?.path === path
        ? { path, file: { ...previous.file, etag: data.etag } }
        : previous,
    );
  }, [query.data, path]);

  useEffect(() => {
    setDiskChange(null);
    const unsubscribe = eventClient.subscribe<WorkspaceEvent>(
      'workspace.event',
      (event) => {
        const payload = event.payload;
        if (!payload) return;
        const eventWorkspaceId = event.workspace_id ?? payload.workspace_id;
        if (eventWorkspaceId !== workspaceId) return;
        if (payload.path !== path && payload.old_path !== path) return;

        const change = changeMessages[payload.type];
        if (change) setDiskChange(change);
      },
    );
    return unsubscribe;
  }, [workspaceId, path]);

  const file = cache?.path === path ? cache.file : null;

  const reload = () => {
    setDiskChange(null);
    setRefreshToken((value) => value + 1);
  };

  if (query.isLoading && !file) {
    return (
      <div className="flex h-full items-center justify-center gap-3 text-xs text-slate-500">
        <Spinner size="sm" label="Loading file" />
        Loading file…
      </div>
    );
  }

  if (query.isError && !file) {
    return (
      <div className="p-4">
        <ErrorState
          error={query.error}
          title="Could not read file"
          onRetry={reload}
        />
      </div>
    );
  }

  if (!file) {
    return (
      <EmptyState
        title="Select a file"
        description="Choose a file in the explorer to preview its contents."
      />
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      {diskChange ? (
        <div className="flex shrink-0 flex-col gap-2 border-b border-amber-500/25 bg-amber-500/[0.06] px-3 py-2 sm:flex-row sm:items-center sm:justify-between">
          <div className="min-w-0">
            <div className="text-xs font-medium text-amber-200">{diskChange.title}</div>
            <div className="mt-0.5 text-[11px] text-amber-300/70">{diskChange.detail}</div>
          </div>
          <div className="flex shrink-0 gap-2">
            <Button variant="ghost" size="sm" onClick={() => setDiskChange(null)}>
              Dismiss
            </Button>
            <Button variant="secondary" size="sm" onClick={reload}>
              Reload from disk
            </Button>
          </div>
        </div>
      ) : null}

      {file.truncated ? (
        <div className="shrink-0 border-b border-sky-500/20 bg-sky-500/5 px-3 py-2 text-[11px] text-sky-300/80">
          This preview is truncated by the workspace service. On-disk size: {formatBytes(file.size)}.
        </div>
      ) : null}

      {query.isError ? (
        <div className="flex shrink-0 items-center justify-between gap-3 border-b border-red-500/20 bg-red-500/5 px-3 py-2 text-[11px] text-red-300">
          <span className="truncate">Could not reload: {getErrorMessage(query.error)}</span>
          <button
            type="button"
            onClick={reload}
            className="shrink-0 font-medium text-red-200 hover:text-red-100"
          >
            Retry
          </button>
        </div>
      ) : null}

      <div className="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-slate-800/70 bg-slate-950/25 px-3 py-1.5 text-[10px] text-slate-600">
        <span className="max-w-[50%] truncate font-mono text-slate-400" title={path}>
          {path}
        </span>
        <span>{formatBytes(file.size)}</span>
        <span>{file.encoding}</span>
        <span className="hidden sm:inline">read-only</span>
        <span className="hidden truncate font-mono sm:inline" title={file.etag}>
          etag {file.etag}
        </span>
        <div className="ml-auto flex items-center gap-2">
          {query.isFetching ? <Spinner size="sm" label="Reloading file" /> : null}
          <Button variant="ghost" size="sm" onClick={reload} className="h-6 px-2 text-[10px]">
            Reload
          </Button>
        </div>
      </div>

      {file.is_binary ? (
        <EmptyState
          title="Binary file"
          description="This file is not suitable for a text preview. Its content has not been loaded into the editor."
          className="flex-1"
        />
      ) : (
        <div className="min-h-0 flex-1">
          <CodeEditor
            path={`workspace://${workspaceId}/${path}`}
            value={file.content}
            language={languageForPath(path, file.language)}
          />
        </div>
      )}
    </div>
  );
}
