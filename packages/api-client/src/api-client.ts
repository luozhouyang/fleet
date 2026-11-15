import type {
  Ack,
  AgentMessage,
  AgentPermissionRequest,
  AgentSession,
  CreateSessionRequest,
  CreateWorkspaceRequest,
  DirectoryListing,
  FileContent,
  GitBranch,
  GitCheckoutRequest,
  GitCheckoutResult,
  GitCommitRequest,
  GitCommitResult,
  GitDiff,
  GitDiffOptions,
  GitDiffSummary,
  GitLogEntry,
  GitLogRequest,
  GitPathsRequest,
  GitStatus,
  ListDirectoryRequest,
  Machine,
  NotModified,
  ProviderInfo,
  RevokedMachine,
  SearchRequest,
  SearchResponse,
  SendMessageRequest,
  SessionDetail,
  Workspace,
} from '@agent-fleet/protocol-types';
import { ApiClientError, errorFromResponse } from './errors';

type QueryValue = string | number | boolean | null | undefined;

export interface ApiClientOptions {
  /** Base URL including `/api/v1`, e.g. `/api/v1`. */
  baseUrl?: string;
  /** A static token or a getter evaluated for every request. */
  token?: string | (() => string | null | undefined) | null;
  /** Optional dedicated Git write token sent as `X-Agent-Fleet-Git-Token`. */
  gitWriteToken?: string | (() => string | null | undefined) | null;
  timeoutMs?: number;
  fetch?: typeof globalThis.fetch;
  requestIdFactory?: () => string;
  /** Invoked whenever the server rejects a request with HTTP 401. */
  onUnauthorized?: () => void;
}

export interface ApiCallOptions {
  signal?: AbortSignal;
  timeoutMs?: number;
  requestId?: string;
  headers?: HeadersInit;
}

export interface ListSessionsOptions extends ApiCallOptions {
  /** Include terminated/archived sessions. The server excludes them by default. */
  includeClosed?: boolean;
}

export interface FileReadOptions extends ApiCallOptions {
  etag?: string;
  maxBytes?: number;
}

interface RequestOptions extends ApiCallOptions {
  query?: Record<string, QueryValue>;
}

interface OpenResponse {
  response: Response;
  requestId: string;
  controller: AbortController;
  signal?: AbortSignal;
  timeoutMs: number;
  timedOut: () => boolean;
  cleanup: () => void;
}

const DEFAULT_TIMEOUT_MS = 30_000;

function randomRequestId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return `req_${crypto.randomUUID()}`;
  }
  return `req_${Date.now().toString(36)}_${Math.random().toString(36).slice(2)}`;
}

function appendQuery(url: URL, query?: Record<string, QueryValue>): void {
  if (!query) return;
  for (const [key, value] of Object.entries(query)) {
    if (value === undefined || value === null) continue;
    url.searchParams.set(key, String(value));
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function isNotModified(value: unknown): value is NotModified {
  return (
    isRecord(value) &&
    typeof value.path === 'string' &&
    typeof value.etag === 'string' &&
    !('content' in value)
  );
}

export class ApiClient {
  private readonly baseUrl: string;
  private readonly tokenOption: ApiClientOptions['token'];
  private readonly gitWriteTokenOption: ApiClientOptions['gitWriteToken'];
  private readonly timeoutMs: number;
  private readonly fetchImpl: typeof globalThis.fetch;
  private readonly requestIdFactory: () => string;
  private readonly onUnauthorized?: () => void;

  constructor(options: ApiClientOptions = {}) {
    this.baseUrl = (options.baseUrl ?? '/api/v1').replace(/\/+$/, '');
    this.tokenOption = options.token;
    this.gitWriteTokenOption = options.gitWriteToken;
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    this.fetchImpl = options.fetch ?? globalThis.fetch.bind(globalThis);
    this.requestIdFactory = options.requestIdFactory ?? randomRequestId;
    this.onUnauthorized = options.onUnauthorized;
  }

  // Machines
  listMachines(options: ApiCallOptions = {}): Promise<Machine[]> {
    return this.request<Machine[]>('GET', '/machines', undefined, options);
  }

  getMachine(id: string, options: ApiCallOptions = {}): Promise<Machine> {
    return this.request<Machine>('GET', `/machines/${encodeURIComponent(id)}`, undefined, options);
  }

  deleteMachine(id: string, options: ApiCallOptions = {}): Promise<void> {
    return this.request<void>(
      'DELETE',
      `/machines/${encodeURIComponent(id)}`,
      undefined,
      options,
    );
  }

  getMachineProviders(id: string, options: ApiCallOptions = {}): Promise<ProviderInfo[]> {
    return this.request<ProviderInfo[]>(
      'GET',
      `/machines/${encodeURIComponent(id)}/providers`,
      undefined,
      options,
    );
  }

  /** Lists machines that were permanently revoked and can be unrevoked. */
  listRevokedMachines(options: ApiCallOptions = {}): Promise<RevokedMachine[]> {
    return this.request<RevokedMachine[]>('GET', '/machines/revoked', undefined, options);
  }

  /** Restores a previously revoked machine. */
  unrevokeMachine(id: string, options: ApiCallOptions = {}): Promise<Ack | void> {
    return this.request<Ack | void>(
      'POST',
      `/machines/${encodeURIComponent(id)}/unrevoke`,
      {},
      options,
    );
  }

  // Workspaces
  listWorkspaces(machineId?: string, options: ApiCallOptions = {}): Promise<Workspace[]> {
    return this.request<Workspace[]>(
      'GET',
      '/workspaces',
      undefined,
      { ...options, query: { machine_id: machineId } },
    );
  }

  createWorkspace(
    body: CreateWorkspaceRequest,
    options: ApiCallOptions = {},
  ): Promise<Workspace> {
    return this.request<Workspace>('POST', '/workspaces', body, options);
  }

  deleteWorkspace(id: string, options: ApiCallOptions = {}): Promise<void> {
    return this.request<void>(
      'DELETE',
      `/workspaces/${encodeURIComponent(id)}`,
      undefined,
      options,
    );
  }

  getTree(
    workspaceId: string,
    listing: ListDirectoryRequest = {},
    options: ApiCallOptions = {},
  ): Promise<DirectoryListing> {
    return this.request<DirectoryListing>(
      'GET',
      `/workspaces/${encodeURIComponent(workspaceId)}/tree`,
      undefined,
      {
        ...options,
        query: {
          path: listing.path,
          cursor: listing.cursor,
          limit: listing.limit,
          show_ignored: listing.show_ignored,
        },
      },
    );
  }

  /**
   * Reads a file, sending both `If-None-Match` and the query `etag` value.
   * A 304 or a `NotModified` JSON body is returned as `NotModified`.
   */
  async readFile(
    workspaceId: string,
    path: string,
    options: FileReadOptions = {},
  ): Promise<FileContent | NotModified> {
    const headers = new Headers();
    if (options.etag) headers.set('If-None-Match', options.etag);
    const open = await this.send(
      `/workspaces/${encodeURIComponent(workspaceId)}/files`,
      { method: 'GET', headers },
      {
        ...options,
        query: { path, etag: options.etag, max_bytes: options.maxBytes },
      },
    );
    try {
      const { response, requestId } = open;
      if (response.status === 304) {
        return {
          path,
          etag: response.headers.get('etag') ?? options.etag ?? '',
        };
      }

      const data = await this.parseBody(response, requestId);
      if (isNotModified(data)) return data;
      if (!isRecord(data) || typeof data.content !== 'string') {
        throw new ApiClientError({
          code: 'protocol_error',
          message: 'File response did not contain content.',
          requestId,
          details: data,
        });
      }
      return data as unknown as FileContent;
    } catch (error) {
      throw this.bodyError(error, open);
    } finally {
      open.cleanup();
    }
  }

  search(
    workspaceId: string,
    search: SearchRequest,
    options: ApiCallOptions = {},
  ): Promise<SearchResponse> {
    return this.request<SearchResponse>(
      'GET',
      `/workspaces/${encodeURIComponent(workspaceId)}/search`,
      undefined,
      {
        ...options,
        query: {
          q: search.query,
          path: search.path,
          max_results: search.max_results,
          case_sensitive: search.case_sensitive,
        },
      },
    );
  }

  // Git
  gitStatus(workspaceId: string, options: ApiCallOptions = {}): Promise<GitStatus> {
    return this.request<GitStatus>(
      'GET',
      `/workspaces/${encodeURIComponent(workspaceId)}/git/status`,
      undefined,
      options,
    );
  }

  gitDiff(
    workspaceId: string,
    diffOptions: GitDiffOptions = {},
    options: ApiCallOptions = {},
  ): Promise<GitDiff> {
    return this.request<GitDiff>(
      'GET',
      `/workspaces/${encodeURIComponent(workspaceId)}/git/diff`,
      undefined,
      { ...options, query: this.diffQuery(diffOptions) },
    );
  }

  gitDiffSummary(
    workspaceId: string,
    diffOptions: GitDiffOptions = {},
    options: ApiCallOptions = {},
  ): Promise<GitDiffSummary> {
    return this.request<GitDiffSummary>(
      'GET',
      `/workspaces/${encodeURIComponent(workspaceId)}/git/diff/summary`,
      undefined,
      { ...options, query: this.diffQuery(diffOptions) },
    );
  }

  gitBranches(workspaceId: string, options: ApiCallOptions = {}): Promise<GitBranch[]> {
    return this.request<GitBranch[]>(
      'GET',
      `/workspaces/${encodeURIComponent(workspaceId)}/git/branches`,
      undefined,
      options,
    );
  }

  gitLog(
    workspaceId: string,
    log: GitLogRequest = {},
    options: ApiCallOptions = {},
  ): Promise<GitLogEntry[]> {
    return this.request<GitLogEntry[]>(
      'GET',
      `/workspaces/${encodeURIComponent(workspaceId)}/git/log`,
      undefined,
      {
        ...options,
        query: { limit: log.limit, path: log.path, reference: log.reference },
      },
    );
  }

  gitStage(
    workspaceId: string,
    request: GitPathsRequest,
    options: ApiCallOptions = {},
  ): Promise<Ack> {
    return this.request<Ack>(
      'POST',
      `/workspaces/${encodeURIComponent(workspaceId)}/git/stage`,
      request,
      this.withWriteToken(options),
    );
  }

  gitUnstage(
    workspaceId: string,
    request: GitPathsRequest,
    options: ApiCallOptions = {},
  ): Promise<Ack> {
    return this.request<Ack>(
      'POST',
      `/workspaces/${encodeURIComponent(workspaceId)}/git/unstage`,
      request,
      this.withWriteToken(options),
    );
  }

  gitCommit(
    workspaceId: string,
    request: GitCommitRequest,
    options: ApiCallOptions = {},
  ): Promise<GitCommitResult> {
    return this.request<GitCommitResult>(
      'POST',
      `/workspaces/${encodeURIComponent(workspaceId)}/git/commit`,
      request,
      this.withWriteToken(options),
    );
  }

  gitCheckout(
    workspaceId: string,
    request: GitCheckoutRequest,
    options: ApiCallOptions = {},
  ): Promise<GitCheckoutResult> {
    return this.request<GitCheckoutResult>(
      'POST',
      `/workspaces/${encodeURIComponent(workspaceId)}/git/checkout`,
      request,
      this.withWriteToken(options),
    );
  }

  // Sessions
  listSessions(options: ListSessionsOptions = {}): Promise<AgentSession[]> {
    const { includeClosed, ...rest } = options;
    return this.request<AgentSession[]>('GET', '/sessions', undefined, {
      ...rest,
      query: { include_closed: includeClosed ? true : undefined },
    });
  }

  createSession(body: CreateSessionRequest, options: ApiCallOptions = {}): Promise<AgentSession> {
    return this.request<AgentSession>('POST', '/sessions', body, options);
  }

  getSession(id: string, options: ApiCallOptions = {}): Promise<SessionDetail> {
    return this.request<SessionDetail>(
      'GET',
      `/sessions/${encodeURIComponent(id)}`,
      undefined,
      options,
    );
  }

  /** Archives a session: its history is kept and it is hidden from the default list. */
  closeSession(id: string, options: ApiCallOptions = {}): Promise<AgentSession | Ack | void> {
    return this.request<AgentSession | Ack | void>(
      'POST',
      `/sessions/${encodeURIComponent(id)}/close`,
      {},
      options,
    );
  }

  /** Permanently deletes a session and its recorded history. */
  deleteSession(id: string, options: ApiCallOptions = {}): Promise<void> {
    return this.request<void>('DELETE', `/sessions/${encodeURIComponent(id)}`, undefined, options);
  }

  sendMessage(
    id: string,
    body: SendMessageRequest,
    options: ApiCallOptions = {},
  ): Promise<Ack | AgentMessage> {
    return this.request<Ack | AgentMessage>(
      'POST',
      `/sessions/${encodeURIComponent(id)}/messages`,
      body,
      options,
    );
  }

  interruptSession(
    id: string,
    options: ApiCallOptions = {},
  ): Promise<AgentSession | Ack | void> {
    return this.request<AgentSession | Ack | void>(
      'POST',
      `/sessions/${encodeURIComponent(id)}/interrupt`,
      {},
      options,
    );
  }

  resumeSession(id: string, options: ApiCallOptions = {}): Promise<AgentSession | Ack | void> {
    return this.request<AgentSession | Ack | void>(
      'POST',
      `/sessions/${encodeURIComponent(id)}/resume`,
      {},
      options,
    );
  }

  // Permissions
  listPermissions(
    sessionId: string,
    options: ApiCallOptions = {},
  ): Promise<AgentPermissionRequest[]> {
    return this.request<AgentPermissionRequest[]>(
      'GET',
      `/sessions/${encodeURIComponent(sessionId)}/permissions`,
      undefined,
      options,
    );
  }

  approvePermission(
    id: string,
    message?: string,
    options: ApiCallOptions = {},
  ): Promise<AgentPermissionRequest | Ack> {
    return this.respondPermission(id, true, message, options);
  }

  rejectPermission(
    id: string,
    message?: string,
    options: ApiCallOptions = {},
  ): Promise<AgentPermissionRequest | Ack> {
    return this.respondPermission(id, false, message, options);
  }

  private respondPermission(
    id: string,
    approve: boolean,
    message?: string,
    options: ApiCallOptions = {},
  ): Promise<AgentPermissionRequest | Ack> {
    return this.request<AgentPermissionRequest | Ack>(
      'POST',
      `/permissions/${encodeURIComponent(id)}/${approve ? 'approve' : 'reject'}`,
      { permission_id: id, approve, message },
      options,
    );
  }

  private diffQuery(options: GitDiffOptions): Record<string, QueryValue> {
    return {
      path: options.path,
      staged: options.staged,
      base: options.base,
      head: options.head,
      include_untracked: options.include_untracked,
      context_lines: options.context_lines,
      ignore_whitespace: options.ignore_whitespace,
    };
  }

  private getToken(): string | null {
    const value =
      typeof this.tokenOption === 'function' ? this.tokenOption() : this.tokenOption;
    return value && value.trim() ? value.trim() : null;
  }

  private withWriteToken(options: ApiCallOptions): ApiCallOptions {
    const value =
      typeof this.gitWriteTokenOption === 'function'
        ? this.gitWriteTokenOption()
        : this.gitWriteTokenOption;
    const token = value?.trim();
    if (!token) return options;
    const headers = new Headers(options.headers);
    headers.set('X-Agent-Fleet-Git-Token', token);
    return { ...options, headers };
  }

  private buildUrl(path: string, query?: Record<string, QueryValue>): string {
    const origin = typeof window !== 'undefined' ? window.location.origin : 'http://localhost';
    const base = path.startsWith('http') ? path : `${this.baseUrl}${path}`;
    const url = new URL(base, origin);
    appendQuery(url, query);
    return url.toString();
  }

  private async send(
    path: string,
    init: RequestInit,
    options: RequestOptions,
  ): Promise<OpenResponse> {
    if (options.signal?.aborted) {
      throw new ApiClientError({
        code: 'request_aborted',
        message: 'The request was aborted.',
        requestId: options.requestId,
      });
    }

    const requestId = options.requestId ?? this.requestIdFactory();
    const headers = new Headers(init.headers);
    headers.set('Accept', 'application/json');
    headers.set('X-Request-Id', requestId);
    if (!headers.has('Content-Type') && init.body !== undefined) {
      headers.set('Content-Type', 'application/json');
    }
    const token = this.getToken();
    if (token) headers.set('Authorization', `Bearer ${token}`);

    const controller = new AbortController();
    let timedOut = false;
    const timeoutMs = options.timeoutMs ?? this.timeoutMs;
    const timeout =
      timeoutMs > 0
        ? setTimeout(() => {
            timedOut = true;
            controller.abort();
          }, timeoutMs)
        : undefined;
    const onAbort = () => controller.abort(options.signal?.reason);
    options.signal?.addEventListener('abort', onAbort, { once: true });

    try {
      const response = await this.fetchImpl(this.buildUrl(path, options.query), {
        ...init,
        headers,
        signal: controller.signal,
      });
      if (!response.ok && response.status !== 304) {
        if (response.status === 401) this.onUnauthorized?.();
        const body = await this.readJson(response);
        throw errorFromResponse(response.status, body, requestId);
      }
      return {
        response,
        requestId,
        controller,
        signal: options.signal,
        timeoutMs,
        timedOut: () => timedOut,
        cleanup: () => {
          if (timeout !== undefined) clearTimeout(timeout);
          options.signal?.removeEventListener('abort', onAbort);
        },
      };
    } catch (error) {
      if (timeout !== undefined) clearTimeout(timeout);
      options.signal?.removeEventListener('abort', onAbort);
      if (error instanceof ApiClientError) throw error;
      if (timedOut) {
        throw new ApiClientError({
          code: 'timeout',
          message: `Request timed out after ${timeoutMs}ms.`,
          requestId,
          cause: error,
        });
      }
      if (options.signal?.aborted || controller.signal.aborted) {
        throw new ApiClientError({
          code: 'request_aborted',
          message: 'The request was aborted.',
          requestId,
          cause: error,
        });
      }
      throw new ApiClientError({
        code: 'network_error',
        message: error instanceof Error ? error.message : 'Network request failed.',
        requestId,
        cause: error,
      });
    }
  }

  // Map errors raised while consuming a response body (timeout/abort/network).
  private bodyError(error: unknown, open: OpenResponse): unknown {
    if (error instanceof ApiClientError) return error;
    if (open.timedOut()) {
      return new ApiClientError({
        code: 'timeout',
        message: `Request timed out after ${open.timeoutMs}ms.`,
        requestId: open.requestId,
        cause: error,
      });
    }
    if (open.signal?.aborted || open.controller.signal.aborted) {
      return new ApiClientError({
        code: 'request_aborted',
        message: 'The request was aborted.',
        requestId: open.requestId,
        cause: error,
      });
    }
    return new ApiClientError({
      code: 'network_error',
      message: error instanceof Error ? error.message : 'Network request failed.',
      requestId: open.requestId,
      cause: error,
    });
  }

  private async request<T>(
    method: string,
    path: string,
    body?: unknown,
    options: RequestOptions = {},
  ): Promise<T> {
    const init: RequestInit = {
      method,
      body: body === undefined ? undefined : JSON.stringify(body),
      headers: options.headers,
    };
    const open = await this.send(path, init, options);
    try {
      const { response, requestId } = open;
      if (response.status === 204) return undefined as T;
      const data = await this.parseBody(response, requestId);
      return data as T;
    } catch (error) {
      throw this.bodyError(error, open);
    } finally {
      open.cleanup();
    }
  }

  private async readJson(response: Response): Promise<unknown> {
    try {
      return await response.json();
    } catch {
      return null;
    }
  }

  private async parseBody(response: Response, requestId: string): Promise<unknown> {
    const text = await response.text();
    if (!text.trim()) return undefined;
    try {
      return JSON.parse(text);
    } catch {
      throw new ApiClientError({
        code: 'invalid_json',
        message: 'The server returned an invalid JSON response.',
        status: response.status,
        requestId,
        details: text.slice(0, 500),
      });
    }
  }
}
