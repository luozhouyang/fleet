import type { GitDiffOptions } from '@agent-fleet/protocol-types';

export const queryKeys = {
  machines: () => ['machines'] as const,
  machine: (id: string) => ['machines', id] as const,
  machineProviders: (id: string) => ['machines', id, 'providers'] as const,
  machineAgents: (id: string) => ['machines', id, 'agents'] as const,
  machineDrivers: (id: string) => ['machines', id, 'drivers'] as const,
  revokedMachines: () => ['machines', 'revoked'] as const,

  /** Root prefix for every workspace query (list, detail, tree, files and Git). */
  workspacesRoot: () => ['workspaces'] as const,
  workspaces: () => ['workspaces', 'list'] as const,
  workspacesForMachine: (machineId: string) => ['workspaces', 'list', machineId] as const,
  workspace: (id: string) => ['workspaces', id] as const,
  tree: (id: string, path: string, showHidden: boolean) =>
    ['workspaces', id, 'tree', path, showHidden] as const,
  file: (id: string, path: string) => ['workspaces', id, 'file', path] as const,
  search: (id: string, query: string, path = '') =>
    ['workspaces', id, 'search', query, path] as const,

  gitStatus: (id: string) => ['workspaces', id, 'git', 'status'] as const,
  gitDiffSummary: (id: string, options: GitDiffOptions = {}) =>
    [
      'workspaces',
      id,
      'git',
      'diff-summary',
      options.path ?? '',
      Boolean(options.staged),
      Boolean(options.include_untracked),
    ] as const,
  gitDiff: (id: string, options: GitDiffOptions = {}) =>
    [
      'workspaces',
      id,
      'git',
      'diff',
      options.path ?? '',
      Boolean(options.staged),
      Boolean(options.include_untracked),
    ] as const,
  gitBranches: (id: string) => ['workspaces', id, 'git', 'branches'] as const,
  gitLog: (id: string, reference?: string) =>
    ['workspaces', id, 'git', 'log', reference ?? 'HEAD'] as const,

  /** Root prefix for invalidating every session list and detail query. */
  sessions: () => ['sessions'] as const,
  /**
   * Paged rail queries. The parameters are part of the key so cache patches
   * and invalidations can target the whole family by prefix.
   */
  sessionRail: (params: Record<string, unknown> = {}) =>
    ['sessions', 'rail', JSON.stringify(params)] as const,
  sessionFacets: () => ['sessions', 'facets'] as const,
  sessionFolders: () => ['sessions', 'folders'] as const,
  sessionList: (includeClosed: boolean) =>
    ['sessions', 'list', includeClosed ? 'all' : 'active'] as const,
  session: (id: string) => ['sessions', id] as const,
  sessionPermissions: (id: string) => ['sessions', id, 'permissions'] as const,
  sessionQuestions: (id: string) => ['sessions', id, 'questions'] as const,
};
