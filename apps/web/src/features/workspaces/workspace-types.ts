export type ExplorerTab =
  | { id: string; kind: 'file'; path: string }
  | {
      id: string;
      kind: 'diff';
      path: string;
      staged: boolean;
      includeUntracked: boolean;
    };

export function makeFileTab(path: string): ExplorerTab {
  return { id: `file:${path}`, kind: 'file', path };
}

export function makeDiffTab(
  path: string,
  staged: boolean,
  includeUntracked: boolean,
): ExplorerTab {
  return {
    id: `diff:${path}:${staged ? 'staged' : 'worktree'}:${includeUntracked ? 'untracked' : 'tracked'}`,
    kind: 'diff',
    path,
    staged,
    includeUntracked,
  };
}
