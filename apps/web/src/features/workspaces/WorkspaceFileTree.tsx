import { EntryType, type SearchMatch, type WorkspaceEntry } from '@agent-fleet/protocol-types';
import { ChevronRight, Eye, EyeOff, File, Folder, FolderOpen, Search } from 'lucide-react';
import { useMemo, useState } from 'react';
import { Button } from '@/components/ui/button';
import { EmptyState, ErrorState, LoadingState, Spinner } from '@/components/ui/feedback';
import { Input } from '@/components/ui/input';
import { useDirectory, useWorkspaceSearch } from '@/lib/query/hooks';
import { cn } from '@/lib/utils/cn';
import { getErrorMessage } from '@/lib/utils/format';

function sortEntries(entries: WorkspaceEntry[]): WorkspaceEntry[] {
  return [...entries].sort((left, right) => {
    const leftFolder = left.entry_type === EntryType.Directory ? 0 : 1;
    const rightFolder = right.entry_type === EntryType.Directory ? 0 : 1;
    if (leftFolder !== rightFolder) return leftFolder - rightFolder;
    return left.name.localeCompare(right.name);
  });
}

function FileRow({
  entry,
  depth,
  selected,
  onOpen,
}: {
  entry: WorkspaceEntry;
  depth: number;
  selected: boolean;
  onOpen: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onOpen}
      title={entry.path}
      data-testid="tree-entry"
      data-path={entry.path}
      className={cn(
        'flex h-[26px] w-full items-center gap-1.5 pr-2 text-left transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/35',
        selected && 'bg-selected',
      )}
      style={{ paddingLeft: 26 + depth * 12 }}
    >
      <File
        className={cn('size-3 shrink-0', selected ? 'text-primary' : 'text-faint')}
        aria-hidden="true"
      />
      <span
        className={cn(
          'truncate text-[12px] leading-5',
          selected ? 'font-medium text-primary' : 'text-body',
        )}
      >
        {entry.name}
      </span>
    </button>
  );
}

function DirectoryNode({
  workspaceId,
  entry,
  depth,
  showHidden,
  selectedPath,
  onOpenFile,
}: {
  workspaceId: string;
  entry: WorkspaceEntry;
  depth: number;
  showHidden: boolean;
  selectedPath?: string;
  onOpenFile: (path: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const listing = useDirectory(workspaceId, entry.path, showHidden, expanded);

  return (
    <div>
      <button
        type="button"
        onClick={() => setExpanded((current) => !current)}
        aria-expanded={expanded}
        title={entry.path}
        data-testid="tree-entry"
        data-path={entry.path}
        className="flex h-[26px] w-full items-center gap-1.5 pr-2 text-left transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/35"
        style={{ paddingLeft: 10 + depth * 12 }}
      >
        <ChevronRight
          className={cn('size-3 shrink-0 text-faint transition-transform', expanded && 'rotate-90')}
          aria-hidden="true"
        />
        {expanded ? (
          <FolderOpen className="size-3 shrink-0 text-faint" aria-hidden="true" />
        ) : (
          <Folder className="size-3 shrink-0 text-faint" aria-hidden="true" />
        )}
        <span className="truncate text-[12px] leading-5 text-body">{entry.name}</span>
      </button>

      {expanded ? (
        listing.isLoading ? (
          <div
            className="flex items-center gap-1.5 py-1 font-mono text-[9px] text-faint"
            style={{ paddingLeft: 44 + depth * 12 }}
          >
            <Spinner className="size-3" />
            Loading…
          </div>
        ) : listing.isError && !listing.data ? (
          <p
            className="py-1 font-mono text-[9px] text-destructive"
            style={{ paddingLeft: 44 + depth * 12 }}
          >
            Could not load
          </p>
        ) : (
          <>
            <DirectoryEntries
              workspaceId={workspaceId}
              entries={listing.data?.pages.flatMap((page) => page.entries) ?? []}
              depth={depth + 1}
              showHidden={showHidden}
              selectedPath={selectedPath}
              onOpenFile={onOpenFile}
            />
            {listing.hasNextPage ? (
              <LoadMoreRow
                depth={depth + 1}
                loading={listing.isFetchingNextPage}
                onClick={() => void listing.fetchNextPage()}
              />
            ) : null}
          </>
        )
      ) : null}
    </div>
  );
}

function LoadMoreRow({
  depth,
  loading,
  onClick,
}: {
  depth: number;
  loading: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={loading}
      className="w-full py-1 text-left font-mono text-[9.5px] text-muted-foreground transition-colors hover:text-foreground disabled:opacity-60"
      style={{ paddingLeft: 26 + depth * 12 }}
    >
      {loading ? 'Loading…' : 'Load more…'}
    </button>
  );
}

function DirectoryEntries({
  workspaceId,
  entries,
  depth,
  showHidden,
  selectedPath,
  onOpenFile,
}: {
  workspaceId: string;
  entries: WorkspaceEntry[];
  depth: number;
  showHidden: boolean;
  selectedPath?: string;
  onOpenFile: (path: string) => void;
}) {
  const sorted = useMemo(() => sortEntries(entries), [entries]);

  return (
    <>
      {sorted.map((entry) =>
        entry.entry_type === EntryType.Directory ? (
          <DirectoryNode
            key={entry.path}
            workspaceId={workspaceId}
            entry={entry}
            depth={depth}
            showHidden={showHidden}
            selectedPath={selectedPath}
            onOpenFile={onOpenFile}
          />
        ) : (
          <FileRow
            key={entry.path}
            entry={entry}
            depth={depth}
            selected={selectedPath === entry.path}
            onOpen={() => onOpenFile(entry.path)}
          />
        ),
      )}
    </>
  );
}

function SearchResults({
  workspaceId,
  query,
  onOpenFile,
}: {
  workspaceId: string;
  query: string;
  onOpenFile: (path: string) => void;
}) {
  const search = useWorkspaceSearch(workspaceId, query, '');
  const matches: SearchMatch[] = search.data?.matches ?? [];

  if (search.isLoading) return <LoadingState label="Searching…" className="py-8" />;
  if (search.isError) {
    return (
      <ErrorState
        title="Search failed"
        message={getErrorMessage(search.error)}
        onRetry={() => void search.refetch()}
        className="py-8"
      />
    );
  }
  if (matches.length === 0) {
    return <EmptyState title="No matches" description="Try a different search." className="py-8" />;
  }

  return (
    <div className="py-1">
      {matches.map((match, index) => (
        <button
          key={`${match.path}:${match.line_number}:${index}`}
          type="button"
          onClick={() => onOpenFile(match.path)}
          title={`${match.path}:${match.line_number}`}
          className="flex w-full flex-col gap-0.5 px-3 py-1.5 text-left transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/35"
        >
          <span className="w-full truncate font-mono text-[9px] text-faint">
            {match.path}:{match.line_number}
          </span>
          <span className="w-full truncate font-mono text-[10px] text-body">
            {match.line.trim()}
          </span>
        </button>
      ))}
    </div>
  );
}

export function WorkspaceFileTree({
  workspaceId,
  selectedPath,
  onOpenFile,
}: {
  workspaceId: string;
  selectedPath?: string;
  onOpenFile: (path: string) => void;
}) {
  const [showHidden, setShowHidden] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState('');
  const root = useDirectory(workspaceId, '', showHidden, true);
  const rootEntries = useMemo(
    () => root.data?.pages.flatMap((page) => page.entries) ?? [],
    [root.data],
  );
  const searching = searchOpen && query.trim().length > 1;

  return (
    <aside className="flex w-[260px] shrink-0 flex-col overflow-hidden rounded-lg border border-border bg-card shadow-card">
      <div className="flex h-10 shrink-0 items-center justify-between border-b border-border pl-3 pr-1.5">
        <span className="font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint">
          FILES
        </span>
        <div className="flex items-center gap-0.5">
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={() => setShowHidden((current) => !current)}
            aria-label={showHidden ? 'Hide hidden files' : 'Show hidden files'}
            aria-pressed={showHidden}
            title={showHidden ? 'Hide hidden files' : 'Show hidden files'}
            className={showHidden ? 'bg-muted text-foreground' : undefined}
          >
            {showHidden ? (
              <EyeOff className="size-3.5" aria-hidden="true" />
            ) : (
              <Eye className="size-3.5" aria-hidden="true" />
            )}
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={() => {
              setSearchOpen((current) => !current);
              if (searchOpen) setQuery('');
            }}
            aria-label={searchOpen ? 'Close file search' : 'Search files'}
            aria-pressed={searchOpen}
            title={searchOpen ? 'Close file search' : 'Search files'}
            className={searchOpen ? 'bg-muted text-foreground' : undefined}
          >
            <Search className="size-3.5" aria-hidden="true" />
          </Button>
        </div>
      </div>

      {searchOpen ? (
        <div className="shrink-0 border-b border-border p-2.5">
          <Input
            autoFocus
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search files…"
            aria-label="Search files"
            className="h-8 text-[11.5px]"
          />
        </div>
      ) : null}

      <div className="min-h-0 flex-1 overflow-y-auto py-1">
        {searching ? (
          <SearchResults workspaceId={workspaceId} query={query} onOpenFile={onOpenFile} />
        ) : root.isLoading ? (
          <LoadingState label="Loading files…" className="py-8" />
        ) : root.isError && !root.data ? (
          <ErrorState
            title="Could not load files"
            message={getErrorMessage(root.error)}
            onRetry={() => void root.refetch()}
            className="py-8"
          />
        ) : rootEntries.length === 0 ? (
          <EmptyState title="Empty directory" className="py-8" />
        ) : (
          <>
            <DirectoryEntries
              workspaceId={workspaceId}
              entries={rootEntries}
              depth={0}
              showHidden={showHidden}
              selectedPath={selectedPath}
              onOpenFile={onOpenFile}
            />
            {root.hasNextPage ? (
              <LoadMoreRow
                depth={0}
                loading={root.isFetchingNextPage}
                onClick={() => void root.fetchNextPage()}
              />
            ) : null}
          </>
        )}
      </div>
    </aside>
  );
}
