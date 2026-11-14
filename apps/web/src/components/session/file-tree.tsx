import { EntryType, type WorkspaceEntry } from '@agent-fleet/protocol-types';
import { ChevronRight, Eye, EyeOff, File, Folder, FolderOpen } from 'lucide-react';
import { useMemo, useState, type ReactNode } from 'react';
import { Spinner } from '@/components/ui/feedback';
import { useDirectory } from '@/lib/query/hooks';
import { cn } from '@/lib/utils/cn';
import { getErrorMessage } from '@/lib/utils/format';

export function FileTree({
  workspaceId,
  selectedPath,
  onSelectPath,
}: {
  workspaceId: string;
  selectedPath: string;
  onSelectPath: (path: string) => void;
}) {
  const [showHidden, setShowHidden] = useState(false);

  return (
    <div>
      <div className="flex items-center justify-between px-2 py-1">
        <span className="font-mono text-[9px] uppercase tracking-[0.12em] text-faint">Files</span>
        <button
          type="button"
          onClick={() => setShowHidden((value) => !value)}
          aria-pressed={showHidden}
          aria-label={showHidden ? 'Hide hidden files' : 'Show hidden files'}
          title={showHidden ? 'Hide hidden files' : 'Show hidden files'}
          className={cn(
            'flex size-6 items-center justify-center rounded transition-colors',
            showHidden
              ? 'bg-muted text-foreground'
              : 'text-muted-foreground hover:bg-muted hover:text-foreground',
          )}
        >
          {showHidden ? (
            <EyeOff className="size-3" aria-hidden="true" />
          ) : (
            <Eye className="size-3" aria-hidden="true" />
          )}
        </button>
      </div>
      <DirectoryLevel
        workspaceId={workspaceId}
        path=""
        depth={0}
        showHidden={showHidden}
        selectedPath={selectedPath}
        onSelectPath={onSelectPath}
      />
    </div>
  );
}

function DirectoryLevel({
  workspaceId,
  path,
  depth,
  showHidden,
  selectedPath,
  onSelectPath,
}: {
  workspaceId: string;
  path: string;
  depth: number;
  showHidden: boolean;
  selectedPath: string;
  onSelectPath: (path: string) => void;
}) {
  const query = useDirectory(workspaceId, path, showHidden);
  const entries = useMemo(() => {
    const list = query.data?.pages.flatMap((page) => page.entries) ?? [];
    return [...list].sort((left, right) => {
      const leftDirectory = left.entry_type === EntryType.Directory ? 0 : 1;
      const rightDirectory = right.entry_type === EntryType.Directory ? 0 : 1;
      if (leftDirectory !== rightDirectory) return leftDirectory - rightDirectory;
      return left.name.localeCompare(right.name, undefined, { sensitivity: 'base' });
    });
  }, [query.data]);

  if (query.isLoading) {
    return (
      <TreeNote depth={depth}>
        <Spinner className="size-3" />
        Loading…
      </TreeNote>
    );
  }

  if (query.isError) {
    return (
      <TreeNote depth={depth} tone="error">
        {getErrorMessage(query.error)}
        <button
          type="button"
          onClick={() => void query.refetch()}
          className="ml-1 font-bold text-primary hover:underline"
        >
          Retry
        </button>
      </TreeNote>
    );
  }

  if (entries.length === 0) {
    return <TreeNote depth={depth}>{path ? 'Empty directory' : 'No files'}</TreeNote>;
  }

  return (
    <div>
      {entries.map((entry) => (
        <TreeNode
          key={entry.path}
          entry={entry}
          workspaceId={workspaceId}
          depth={depth}
          showHidden={showHidden}
          selectedPath={selectedPath}
          onSelectPath={onSelectPath}
        />
      ))}
      {query.hasNextPage ? (
        <button
          type="button"
          onClick={() => void query.fetchNextPage()}
          disabled={query.isFetchingNextPage}
          className="w-full py-1 text-left font-mono text-[9.5px] text-muted-foreground hover:text-foreground disabled:opacity-60"
          style={{ paddingLeft: 10 + depth * 12 }}
        >
          {query.isFetchingNextPage ? 'Loading…' : 'Load more…'}
        </button>
      ) : null}
    </div>
  );
}

function TreeNode({
  entry,
  workspaceId,
  depth,
  showHidden,
  selectedPath,
  onSelectPath,
}: {
  entry: WorkspaceEntry;
  workspaceId: string;
  depth: number;
  showHidden: boolean;
  selectedPath: string;
  onSelectPath: (path: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const isDirectory = entry.entry_type === EntryType.Directory;
  const selected = !isDirectory && selectedPath === entry.path;

  return (
    <div>
      <button
        type="button"
        onClick={() => {
          if (isDirectory) setExpanded((value) => !value);
          else onSelectPath(entry.path);
        }}
        aria-expanded={isDirectory ? expanded : undefined}
        title={entry.path}
        data-testid="session-tree-entry"
        data-path={entry.path}
        className={cn(
          'flex w-full items-center gap-1.5 py-[3px] pr-2 text-left text-[11.5px] transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/40',
          selected
            ? 'bg-selected text-foreground'
            : 'text-body hover:bg-muted hover:text-foreground',
        )}
        style={{ paddingLeft: 8 + depth * 12 }}
      >
        {isDirectory ? (
          <ChevronRight
            className={cn(
              'size-3 shrink-0 text-faint transition-transform',
              expanded && 'rotate-90',
            )}
            aria-hidden="true"
          />
        ) : (
          <span className="w-3 shrink-0" aria-hidden="true" />
        )}
        {isDirectory ? (
          expanded ? (
            <FolderOpen className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
          ) : (
            <Folder className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
          )
        ) : (
          <File className="size-3.5 shrink-0 text-faint" aria-hidden="true" />
        )}
        <span className="truncate">{entry.name}</span>
      </button>

      {isDirectory && expanded ? (
        <DirectoryLevel
          workspaceId={workspaceId}
          path={entry.path}
          depth={depth + 1}
          showHidden={showHidden}
          selectedPath={selectedPath}
          onSelectPath={onSelectPath}
        />
      ) : null}
    </div>
  );
}

function TreeNote({
  depth,
  tone = 'muted',
  children,
}: {
  depth: number;
  tone?: 'muted' | 'error';
  children: ReactNode;
}) {
  return (
    <div
      className={cn(
        'flex items-center gap-1.5 py-1.5 pr-2 text-[10.5px]',
        tone === 'error' ? 'text-destructive' : 'text-faint',
      )}
      style={{ paddingLeft: 10 + depth * 12 }}
    >
      {children}
    </div>
  );
}
