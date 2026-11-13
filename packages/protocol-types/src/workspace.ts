import type { MachineId, ProjectId, Timestamp, WorkspaceId } from './common';
import type { GitFileStatus } from './git';

/** A registered workspace root on a machine. */
export interface Workspace {
  id: WorkspaceId;
  machine_id: MachineId;
  project_id?: ProjectId;
  name: string;
  /** Absolute path on the machine. Never used by the browser as a path input. */
  root_path: string;
  is_git_repository: boolean;
  created_at: Timestamp;
  updated_at: Timestamp;
}

/** Lightweight workspace descriptor embedded in machine descriptions. */
export interface WorkspaceSummary {
  id: WorkspaceId;
  name: string;
  root_path: string;
  is_git_repository: boolean;
}

/** Request payload for `workspace.register`. */
export interface RegisterWorkspaceRequest {
  root_path: string;
  name?: string;
  project_id?: ProjectId;
}

/** Browser request body for `POST /workspaces`. */
export interface CreateWorkspaceRequest extends RegisterWorkspaceRequest {
  machine_id: MachineId;
}

export enum EntryType {
  File = 'file',
  Directory = 'directory',
  Symlink = 'symlink',
}

/** One entry in a directory listing. */
export interface WorkspaceEntry {
  /** Workspace-relative path using `/` separators. */
  path: string;
  name: string;
  entry_type: EntryType;
  size?: number;
  modified_at?: Timestamp;
  git_status?: GitFileStatus;
  ignored?: boolean;
}

/** Response for `workspace.tree.list`. */
export interface DirectoryListing {
  path: string;
  entries: WorkspaceEntry[];
  next_cursor?: string;
}

export interface ListDirectoryRequest {
  path?: string;
  cursor?: string;
  limit?: number;
  show_ignored?: boolean;
}

/** Metadata for a file without its content. */
export interface FileMetadata {
  path: string;
  size: number;
  modified_at?: Timestamp;
  is_binary: boolean;
  etag: string;
}

/** File content returned to the browser. */
export interface FileContent {
  path: string;
  content: string;
  encoding: string;
  size: number;
  etag: string;
  truncated: boolean;
  is_binary: boolean;
  language?: string;
  modified_at?: Timestamp;
}

export interface ReadFileRequest {
  path: string;
  max_bytes?: number;
  etag?: string;
}

/** Response when the client already holds the current content. */
export interface NotModified {
  path: string;
  etag: string;
}

export interface SearchRequest {
  query: string;
  path?: string;
  max_results?: number;
  case_sensitive?: boolean;
}

export interface SearchMatch {
  path: string;
  line_number: number;
  line: string;
  column?: number;
}

export interface SearchResponse {
  matches: SearchMatch[];
  truncated: boolean;
}

export const WORKSPACE_EVENT_TYPES = [
  'workspace.directory.changed',
  'workspace.file.created',
  'workspace.file.modified',
  'workspace.file.deleted',
  'workspace.file.renamed',
] as const;

export type WorkspaceEventType = (typeof WORKSPACE_EVENT_TYPES)[number];

export interface WorkspaceEvent {
  event_id: string;
  workspace_id: WorkspaceId;
  type: WorkspaceEventType;
  path?: string;
  old_path?: string;
  timestamp: Timestamp;
  payload?: unknown;
}
