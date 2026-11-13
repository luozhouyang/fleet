import {
  EntryType,
  type DirectoryListing,
  type WorkspaceEntry,
} from '@agent-fleet/protocol-types';
import { Button, Spinner, cn } from '@agent-fleet/ui';
import { useInfiniteQuery } from '@tanstack/react-query';
import { useMemo, useState } from 'react';
import { GitStatusLetter } from '@/components/status/StatusBadge';
import { apiClient } from '@/lib/api/client';
import { queryKeys } from '@/lib/query/keys';
import { formatBytes, getErrorMessage } from '@/lib/utils/format';

const PAGE_SIZE = 200;

interface TreeProps {
  workspaceId: string;
  selectedPath: string | null;
  onOpenFile: (path: string) => void;
  showHidden: boolean;
}

export function DirectoryTree({
  workspaceId,
  selectedPath,
  onOpenFile,
  showHidden,
  onShowHiddenChange,
}: TreeProps & { onShowHiddenChange: (value: boolean) => void }) {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex items-center justify-between border-b border-slate-800/70 px-3 py-2">
        <label className="flex cursor-pointer items-center gap-2 text-[11px] text-slate-500">
          <input
            type="checkbox"
            checked={showHidden}
            onChange={(event) => onShowHiddenChange(event.target.checked)}
            className="size-3.5 rounded border-slate-700 bg-slate-950 accent-teal-500"
          />
          Show hidden
        </label>
        <span className="font-mono text-[10px] text-slate-600">lazy tree</span>
      </div>
      <div className="min-h-0 flex-1 overflow-auto py-1">
        <DirectoryListing
          workspaceId={workspaceId}
          path=""
          depth={0}
          showHidden={showHidden}
          selectedPath={selectedPath}
          onOpenFile={onOpenFile}
        />
      </div>
    </div>
  );
}

function DirectoryListing({
  workspaceId,
  path,
  depth,
  showHidden,
  selectedPath,
  onOpenFile,
}: TreeProps & { path: string; depth: number }) {
  const query = useInfiniteQuery({
    queryKey: queryKeys.tree(workspaceId, path, showHidden),
    queryFn: ({ pageParam, signal }) =>
      apiClient.getTree(
        workspaceId,
        {
          path,
          cursor: pageParam,
          limit: PAGE_SIZE,
          show_ignored: showHidden,
        },
        { signal },
      ),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (lastPage: DirectoryListing) => lastPage.next_cursor,
    enabled: Boolean(workspaceId),
  });

  const entries = useMemo(() => {
    const all = query.data?.pages.flatMap((page) => page.entries) ?? [];
    const filtered = showHidden
      ? all
      : all.filter((entry) => !entry.name.startsWith('.'));
    return [...filtered].sort((left, right) => {
      const leftDir = left.entry_type === EntryType.Directory ? 0 : 1;
      const rightDir = right.entry_type === EntryType.Directory ? 0 : 1;
      if (leftDir !== rightDir) return leftDir - rightDir;
      return left.name.localeCompare(right.name, undefined, { sensitivity: 'base' });
    });
  }, [query.data, showHidden]);

  if (query.isLoading) {
    return (
      <div
        className="flex items-center gap-2 py-2 text-[11px] text-slate-600"
        style={{ paddingLeft: 10 + depth * 14 }}
      >
        <Spinner size="sm" label="Loading directory" />
        Loading…
      </div>
    );
  }

  if (query.isError) {
    return (
      <div
        className="py-2 pr-2 text-[11px] text-red-400"
        style={{ paddingLeft: 10 + depth * 14 }}
      >
        {getErrorMessage(query.error)}
        <button
          type="button"
          onClick={() => void query.refetch()}
          className="ml-2 text-teal-300 hover:text-teal-200"
        >
          Retry
        </button>
      </div>
    );
  }

  return (
    <>
      {entries.length === 0 ? (
        <div
          className="py-1.5 text-[11px] italic text-slate-600"
          style={{ paddingLeft: 10 + depth * 14 }}
        >
          Empty directory
        </div>
      ) : (
        entries.map((entry) => (
          <DirectoryNode
            key={entry.path}
            entry={entry}
            depth={depth}
            workspaceId={workspaceId}
            showHidden={showHidden}
            selectedPath={selectedPath}
            onOpenFile={onOpenFile}
          />
        ))
      )}
      {query.hasNextPage ? (
        <div style={{ paddingLeft: 10 + depth * 14 }} className="py-1 pr-2">
          <Button
            variant="ghost"
            size="sm"
            loading={query.isFetchingNextPage}
            onClick={() => void query.fetchNextPage()}
            className="h-6 px-2 text-[10px]"
          >
            Load more
          </Button>
        </div>
      ) : null}
    </>
  );
}

function DirectoryNode({
  entry,
  depth,
  workspaceId,
  showHidden,
  selectedPath,
  onOpenFile,
}: TreeProps & { entry: WorkspaceEntry; depth: number }) {
  const isDirectory = entry.entry_type === EntryType.Directory;
  const [expanded, setExpanded] = useState(false);

  return (
    <div>
      <button
        type="button"
        onClick={() => {
          if (isDirectory) setExpanded((value) => !value);
          else onOpenFile(entry.path);
        }}
        aria-expanded={isDirectory ? expanded : undefined}
        title={entry.path}
        data-testid="tree-entry"
        data-path={entry.path}
        className={cn(
          'flex w-full items-center gap-1.5 py-1 pr-2 text-left text-xs transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-teal-400/50',
          selectedPath === entry.path
            ? 'bg-teal-500/10 text-teal-200'
            : 'text-slate-400 hover:bg-slate-800/50 hover:text-slate-200',
        )}
        style={{ paddingLeft: 8 + depth * 14 }}
      >
        {isDirectory ? (
          <svg
            viewBox="0 0 16 16"
            fill="none"
            className={cn('size-3 shrink-0 text-slate-500 transition-transform', expanded && 'rotate-90')}
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
        ) : (
          <span className="w-3 shrink-0" />
        )}

        {isDirectory ? <FolderIcon /> : <FileIcon name={entry.name} />}

        <span className="truncate">{entry.name}</span>
        {entry.git_status ? <GitStatusLetter status={entry.git_status} /> : null}
        {!isDirectory && entry.size !== undefined ? (
          <span className="ml-auto shrink-0 pl-2 font-mono text-[9px] text-slate-600">
            {formatBytes(entry.size)}
          </span>
        ) : null}
      </button>

      {isDirectory && expanded ? (
        <DirectoryListing
          workspaceId={workspaceId}
          path={entry.path}
          depth={depth + 1}
          showHidden={showHidden}
          selectedPath={selectedPath}
          onOpenFile={onOpenFile}
        />
      ) : null}
    </div>
  );
}

function FolderIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" className="size-3.5 shrink-0 text-amber-300/70" aria-hidden="true">
      <path
        d="M1.5 4.5A1.5 1.5 0 0 1 3 3h3l1.5 1.5H13A1.5 1.5 0 0 1 14.5 6v5.5A1.5 1.5 0 0 1 13 13H3a1.5 1.5 0 0 1-1.5-1.5v-7Z"
        stroke="currentColor"
        strokeWidth="1.2"
        strokeLinejoin="round"
      />
    </svg>
  );
}

const extensionColors: Record<string, string> = {
  rs: 'text-orange-300/80',
  ts: 'text-sky-300/80',
  tsx: 'text-sky-300/80',
  js: 'text-yellow-300/80',
  jsx: 'text-yellow-300/80',
  json: 'text-amber-300/70',
  md: 'text-slate-300/80',
  css: 'text-blue-300/80',
  toml: 'text-rose-300/70',
  yaml: 'text-emerald-300/70',
  yml: 'text-emerald-300/70',
};

function FileIcon({ name }: { name: string }) {
  const extension = name.toLowerCase().split('.').pop() ?? '';
  return (
    <svg
      viewBox="0 0 16 16"
      fill="none"
      className={cn('size-3.5 shrink-0', extensionColors[extension] ?? 'text-slate-500')}
      aria-hidden="true"
    >
      <path
        d="M4 1.5h5L12.5 5v9.5H4V1.5Z"
        stroke="currentColor"
        strokeWidth="1.2"
        strokeLinejoin="round"
      />
      <path d="M9 1.5V5h3.5" stroke="currentColor" strokeWidth="1.2" strokeLinejoin="round" />
    </svg>
  );
}
