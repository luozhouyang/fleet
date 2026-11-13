import type { Timestamp } from './common';

export enum GitFileStatus {
  Modified = 'modified',
  Added = 'added',
  Deleted = 'deleted',
  Renamed = 'renamed',
  Copied = 'copied',
  Untracked = 'untracked',
  Conflicted = 'conflicted',
  Ignored = 'ignored',
  Unmodified = 'unmodified',
}

/** One changed file. Index and worktree states are preserved separately. */
export interface GitFileChange {
  /** Workspace-relative path using `/` separators. */
  path: string;
  old_path?: string;
  status: GitFileStatus;
  /** True when the change is in the index (staged). */
  staged: boolean;
  index_status?: string;
  worktree_status?: string;
}

export interface GitStatus {
  branch?: string;
  ahead: number;
  behind: number;
  files?: GitFileChange[];
  head_oid?: string;
  is_clean: boolean;
  detached?: boolean;
}

export interface GitBranch {
  name: string;
  is_current: boolean;
  is_remote: boolean;
  upstream?: string;
  ahead: number;
  behind: number;
  oid?: string;
}

export interface GitLogEntry {
  id: string;
  short_id: string;
  message: string;
  author: string;
  email: string;
  timestamp: Timestamp;
  parents?: string[];
  refs?: string[];
}

/** Repository descriptor for a workspace. */
export interface GitRepository {
  root: string;
  git_dir: string;
  worktree?: string;
}

export enum DiffLineType {
  Context = 'context',
  Added = 'added',
  Deleted = 'deleted',
  NoNewline = 'no_newline',
}

export interface DiffLine {
  line_type: DiffLineType;
  /** Content without the leading `+`, `-` or space marker. */
  content: string;
  old_line_no?: number;
  new_line_no?: number;
}

export interface DiffHunk {
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
  header?: string;
  lines: DiffLine[];
}

/** Full unified diff for one file. */
export interface FileDiff {
  path: string;
  old_path?: string;
  status: GitFileStatus;
  additions: number;
  deletions: number;
  is_binary: boolean;
  hunks: DiffHunk[];
}

export interface GitDiff {
  files: FileDiff[];
  is_binary: boolean;
}

/** Lightweight per-file summary used before fetching full diffs. */
export interface DiffSummary {
  path: string;
  old_path?: string;
  status: GitFileStatus;
  additions: number;
  deletions: number;
  is_binary: boolean;
  staged: boolean;
}

export interface GitDiffSummary {
  files: DiffSummary[];
  branch?: string;
  ahead: number;
  behind: number;
}

/** Options for `git.diff` and `git.diff.summary`. */
export interface GitDiffOptions {
  /** Restrict to a single workspace-relative path. */
  path?: string;
  /** Diff the index against HEAD instead of the worktree against the index. */
  staged?: boolean;
  base?: string;
  head?: string;
  include_untracked?: boolean;
  context_lines?: number;
  ignore_whitespace?: boolean;
}

export interface GitLogRequest {
  limit?: number;
  path?: string;
  reference?: string;
}
