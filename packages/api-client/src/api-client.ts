import type {
  Ack,
  AddMemberRequest,
  AgentCatalogEntry,
  AgentMessage,
  AgentPermissionRequest,
  AgentProbeResult,
  AgentQuestionAnswer,
  AgentQuestionRequest,
  AgentSession,
  AuthOptions,
  CreateInvitationRequest,
  CreateOrganizationRequest,
  CreatePairingCodeRequest,
  CreateSessionRequest,
  CreateUserRequest,
  CreatedOrgInvitation,
  CreatedUser,
  DriverInfo,
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
  MachineTokenRotation,
  Me,
  Membership,
  NotModified,
  OrgInvitation,
  OrgMember,
  OrganizationSummary,
  PairingCode,
  ProviderInfo,
  RealtimeTicket,
  RevokedMachine,
  RotatedUserToken,
  SearchRequest,
  SearchResponse,
  SendMessageRequest,
  SessionDetail,
  UpdateMemberRequest,
  UpdateOrganizationRequest,
  UpdateSessionRequest,
  User,
  Workspace,
} from '@agent-fleet/protocol-types';
import { ApiClientError, errorFromResponse } from './errors';

type QueryValue = string | number | boolean | null | undefined;

export interface ApiClientOptions {
  /** Base URL including `/api/v1`, e.g. `/api/v1`. */
  baseUrl?: string;
  /** A static token or a getter evaluated for every request. */
  token?: string | (() => string | null | undefined) | null;
  /** Optional dedicated Git write token sent as `X-Fleet-Git-Token`. */
  gitWriteToken?: string | (() => string | null | undefined) | null;
  /**
   * Active organization sent as `X-Fleet-Org`. Omitted requests use the
   * server-side default (a team organization, else the personal space).
   */
  orgId?: string | (() => string | null | undefined) | null;
  /**
   * CSRF token sent as `X-CSRF-Token` on unsafe methods. Required for
   * cookie-authenticated writes; the value comes from the `fleet_csrf` cookie.
   */
  csrfToken?: string | (() => string | null | undefined) | null;
  timeoutMs?: number;
  fetch?: typeof globalThis.fetch;
  requestIdFactory?: () => string;
  /** Invoked whenever the server rejects a request with HTTP 401. */
  onUnauthorized?: (error: ApiClientError) => void;
  /** Invoked whenever the server rejects a request with HTTP 403. */
  onForbidden?: (error: ApiClientError) => void;
}

export interface ApiCallOptions {
  signal?: AbortSignal;
  timeoutMs?: number;
  requestId?: string;
  headers?: HeadersInit;
  /**
   * Sent as `Idempotency-Key` on mutating requests. A retry with the same key
   * replays the daemon's cached response instead of executing the command a
   * second time. Generate one per user action and reuse it across retries.
   */
  idempotencyKey?: string;
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
  private readonly orgIdOption: ApiClientOptions['orgId'];
  private readonly csrfTokenOption: ApiClientOptions['csrfToken'];
  private readonly timeoutMs: number;
  private readonly fetchImpl: typeof globalThis.fetch;
  private readonly requestIdFactory: () => string;
  private readonly onUnauthorized?: (error: ApiClientError) => void;
  private readonly onForbidden?: (error: ApiClientError) => void;

  constructor(options: ApiClientOptions = {}) {
    this.baseUrl = (options.baseUrl ?? '/api/v1').replace(/\/+$/, '');
    this.tokenOption = options.token;
    this.gitWriteTokenOption = options.gitWriteToken;
    this.orgIdOption = options.orgId;
    this.csrfTokenOption = options.csrfToken;
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    this.fetchImpl = options.fetch ?? globalThis.fetch.bind(globalThis);
    this.requestIdFactory = options.requestIdFactory ?? randomRequestId;
    this.onUnauthorized = options.onUnauthorized;
    this.onForbidden = options.onForbidden;
  }

  // Identity and users
  /** Describe the authenticated caller. */
  me(options: ApiCallOptions = {}): Promise<Me> {
    return this.request<Me>('GET', '/me', undefined, options);
  }

  /** Public login options: configured OAuth providers and bootstrap state. */
  authProviders(options: ApiCallOptions = {}): Promise<AuthOptions> {
    return this.request<AuthOptions>('GET', '/auth/providers', undefined, options);
  }

  /**
   * Upgrade the current user bearer token into an HttpOnly browser session.
   *
   * On success the server sets the session and CSRF cookies. The bootstrap
   * admin token is rejected: it cannot own a browser session.
   */
  createBrowserSession(options: ApiCallOptions = {}): Promise<Me> {
    return this.request<Me>('POST', '/auth/session', undefined, options);
  }

  /** Revoke the presented browser session and clear its cookies. */
  logout(options: ApiCallOptions = {}): Promise<void> {
    return this.request<void>('POST', '/auth/logout', undefined, options);
  }

  /** List users visible to the caller; administrators only. */
  listUsers(orgId?: string | null, options: ApiCallOptions = {}): Promise<User[]> {
    return this.request<User[]>('GET', '/users', undefined, {
      ...options,
      query: { org_id: orgId },
    });
  }

  /** Organizations the caller belongs to, with its role in each. */
  listOrganizations(options: ApiCallOptions = {}): Promise<OrganizationSummary[]> {
    return this.request<OrganizationSummary[]>('GET', '/orgs', undefined, options);
  }

  getOrganization(id: string, options: ApiCallOptions = {}): Promise<OrganizationSummary> {
    return this.request<OrganizationSummary>(
      'GET',
      `/orgs/${encodeURIComponent(id)}`,
      undefined,
      options,
    );
  }

  createOrganization(
    request: CreateOrganizationRequest,
    options: ApiCallOptions = {},
  ): Promise<OrganizationSummary> {
    return this.request<OrganizationSummary>('POST', '/orgs', request, options);
  }

  renameOrganization(
    id: string,
    request: UpdateOrganizationRequest,
    options: ApiCallOptions = {},
  ): Promise<OrganizationSummary> {
    return this.request<OrganizationSummary>(
      'PATCH',
      `/orgs/${encodeURIComponent(id)}`,
      request,
      options,
    );
  }

  deleteOrganization(id: string, options: ApiCallOptions = {}): Promise<void> {
    return this.request<void>('DELETE', `/orgs/${encodeURIComponent(id)}`, undefined, options);
  }

  listOrganizationMembers(id: string, options: ApiCallOptions = {}): Promise<OrgMember[]> {
    return this.request<OrgMember[]>(
      'GET',
      `/orgs/${encodeURIComponent(id)}/members`,
      undefined,
      options,
    );
  }

  addOrganizationMember(
    id: string,
    request: AddMemberRequest,
    options: ApiCallOptions = {},
  ): Promise<Membership> {
    return this.request<Membership>(
      'POST',
      `/orgs/${encodeURIComponent(id)}/members`,
      request,
      options,
    );
  }

  updateOrganizationMember(
    id: string,
    userId: string,
    request: UpdateMemberRequest,
    options: ApiCallOptions = {},
  ): Promise<Membership> {
    return this.request<Membership>(
      'PATCH',
      `/orgs/${encodeURIComponent(id)}/members/${encodeURIComponent(userId)}`,
      request,
      options,
    );
  }

  removeOrganizationMember(
    id: string,
    userId: string,
    options: ApiCallOptions = {},
  ): Promise<void> {
    return this.request<void>(
      'DELETE',
      `/orgs/${encodeURIComponent(id)}/members/${encodeURIComponent(userId)}`,
      undefined,
      options,
    );
  }

  leaveOrganization(id: string, options: ApiCallOptions = {}): Promise<void> {
    return this.request<void>('POST', `/orgs/${encodeURIComponent(id)}/leave`, {}, options);
  }

  listOrganizationInvitations(id: string, options: ApiCallOptions = {}): Promise<OrgInvitation[]> {
    return this.request<OrgInvitation[]>(
      'GET',
      `/orgs/${encodeURIComponent(id)}/invitations`,
      undefined,
      options,
    );
  }

  createOrganizationInvitation(
    id: string,
    request: CreateInvitationRequest,
    options: ApiCallOptions = {},
  ): Promise<CreatedOrgInvitation> {
    return this.request<CreatedOrgInvitation>(
      'POST',
      `/orgs/${encodeURIComponent(id)}/invitations`,
      request,
      options,
    );
  }

  revokeOrganizationInvitation(
    id: string,
    invitationId: string,
    options: ApiCallOptions = {},
  ): Promise<void> {
    return this.request<void>(
      'DELETE',
      `/orgs/${encodeURIComponent(id)}/invitations/${encodeURIComponent(invitationId)}`,
      undefined,
      options,
    );
  }

  /**
   * Mint a single-use ticket for the browser event socket.
   *
   * The ticket is bound to the caller and to `orgId` (or the active
   * organization). The legacy admin token cannot mint a ticket.
   */
  createRealtimeTicket(
    orgId?: string | null,
    options: ApiCallOptions = {},
  ): Promise<RealtimeTicket> {
    return this.request<RealtimeTicket>('POST', '/realtime/ticket', undefined, {
      ...options,
      query: { org_id: orgId },
    });
  }

  /**
   * Create a single-use machine pairing code for the active organization.
   * The plaintext code is returned once; administrators only.
   */
  createPairingCode(
    request: CreatePairingCodeRequest = {},
    options: ApiCallOptions = {},
  ): Promise<PairingCode> {
    return this.request<PairingCode>('POST', '/pairing/codes', request, options);
  }

  /** Create a user and receive its bearer token exactly once. */
  createUser(request: CreateUserRequest, options: ApiCallOptions = {}): Promise<CreatedUser> {
    return this.request<CreatedUser>('POST', '/users', request, options);
  }

  /** Delete a user. Administrators cannot delete themselves. */
  deleteUser(id: string, options: ApiCallOptions = {}): Promise<void> {
    return this.request<void>('DELETE', `/users/${encodeURIComponent(id)}`, undefined, options);
  }

  /** Disable an account and revoke its browser sessions. */
  disableUser(id: string, options: ApiCallOptions = {}): Promise<User> {
    return this.request<User>('POST', `/users/${encodeURIComponent(id)}/disable`, {}, options);
  }

  /** Restore a disabled account. Its previous bearer token becomes valid again. */
  enableUser(id: string, options: ApiCallOptions = {}): Promise<User> {
    return this.request<User>('POST', `/users/${encodeURIComponent(id)}/enable`, {}, options);
  }

  /**
   * Rotate a user's bearer token. `expiresInDays` overrides the configured
   * lifetime; omit it to use the server default, or pass `0` for no expiry.
   */
  rotateUserToken(
    id: string,
    expiresInDays?: number | null,
    options: ApiCallOptions = {},
  ): Promise<RotatedUserToken> {
    return this.request<RotatedUserToken>(
      'POST',
      `/users/${encodeURIComponent(id)}/rotate-token`,
      undefined,
      { ...options, query: { expires_in_days: expiresInDays } },
    );
  }

  // Machines
  listMachines(options: ApiCallOptions = {}): Promise<Machine[]> {
    return this.request<Machine[]>('GET', '/machines', undefined, options);
  }

  getMachine(id: string, options: ApiCallOptions = {}): Promise<Machine> {
    return this.request<Machine>('GET', `/machines/${encodeURIComponent(id)}`, undefined, options);
  }

  deleteMachine(id: string, options: ApiCallOptions = {}): Promise<void> {
    return this.request<void>('DELETE', `/machines/${encodeURIComponent(id)}`, undefined, options);
  }

  /**
   * Rotate a machine token over its live connection.
   *
   * The daemon must be online and confirms after persisting the token. The
   * token is never returned to the browser.
   */
  rotateMachineToken(id: string, options: ApiCallOptions = {}): Promise<MachineTokenRotation> {
    return this.request<MachineTokenRotation>(
      'POST',
      `/machines/${encodeURIComponent(id)}/rotate-token`,
      {},
      options,
    );
  }

  /** Abandon an unconfirmed machine token rotation. */
  cancelMachineTokenRotation(id: string, options: ApiCallOptions = {}): Promise<Ack> {
    return this.request<Ack>(
      'DELETE',
      `/machines/${encodeURIComponent(id)}/rotate-token`,
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

  /** Agent catalog for one connected machine. */
  listAgents(machineId: string, options: ApiCallOptions = {}): Promise<AgentCatalogEntry[]> {
    return this.request<AgentCatalogEntry[]>('GET', '/agents', undefined, {
      ...options,
      query: { machine_id: machineId },
    });
  }

  /** Driver catalog for one connected machine. */
  listDrivers(machineId: string, options: ApiCallOptions = {}): Promise<DriverInfo[]> {
    return this.request<DriverInfo[]>('GET', '/drivers', undefined, {
      ...options,
      query: { machine_id: machineId },
    });
  }

  /** Probe one agent definition without creating a session. */
  probeAgent(
    machineId: string,
    agentId: string,
    options: ApiCallOptions = {},
  ): Promise<AgentProbeResult> {
    return this.request<AgentProbeResult>(
      'POST',
      `/agents/${encodeURIComponent(agentId)}/probe`,
      undefined,
      { ...options, query: { machine_id: machineId } },
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
    return this.request<Workspace[]>('GET', '/workspaces', undefined, {
      ...options,
      query: { machine_id: machineId },
    });
  }

  createWorkspace(body: CreateWorkspaceRequest, options: ApiCallOptions = {}): Promise<Workspace> {
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
  updateSession(
    id: string,
    body: UpdateSessionRequest,
    options: ApiCallOptions = {},
  ): Promise<AgentSession> {
    return this.request<AgentSession>(
      'PATCH',
      `/sessions/${encodeURIComponent(id)}`,
      body,
      options,
    );
  }

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

  interruptSession(id: string, options: ApiCallOptions = {}): Promise<AgentSession | Ack | void> {
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
    sessionId: string,
    permissionId: string,
    optionId?: string,
    message?: string,
    options: ApiCallOptions = {},
  ): Promise<AgentPermissionRequest | Ack> {
    return this.request<AgentPermissionRequest | Ack>(
      'POST',
      `/sessions/${encodeURIComponent(sessionId)}/permissions/${encodeURIComponent(permissionId)}/approve`,
      { option_id: optionId, message },
      options,
    );
  }

  rejectPermission(
    sessionId: string,
    permissionId: string,
    optionId?: string,
    message?: string,
    options: ApiCallOptions = {},
  ): Promise<AgentPermissionRequest | Ack> {
    return this.request<AgentPermissionRequest | Ack>(
      'POST',
      `/sessions/${encodeURIComponent(sessionId)}/permissions/${encodeURIComponent(permissionId)}/reject`,
      { option_id: optionId, message },
      options,
    );
  }

  // Questions
  listQuestions(sessionId: string, options: ApiCallOptions = {}): Promise<AgentQuestionRequest[]> {
    return this.request<AgentQuestionRequest[]>(
      'GET',
      `/sessions/${encodeURIComponent(sessionId)}/questions`,
      undefined,
      options,
    );
  }

  answerQuestion(
    sessionId: string,
    questionId: string,
    answers: AgentQuestionAnswer[],
    options: ApiCallOptions = {},
  ): Promise<Ack | AgentQuestionRequest> {
    return this.request<Ack | AgentQuestionRequest>(
      'POST',
      `/sessions/${encodeURIComponent(sessionId)}/questions/${encodeURIComponent(questionId)}/answer`,
      { answers },
      options,
    );
  }

  rejectQuestion(
    sessionId: string,
    questionId: string,
    message?: string,
    options: ApiCallOptions = {},
  ): Promise<Ack | AgentQuestionRequest> {
    return this.request<Ack | AgentQuestionRequest>(
      'POST',
      `/sessions/${encodeURIComponent(sessionId)}/questions/${encodeURIComponent(questionId)}/reject`,
      { message },
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
    const value = typeof this.tokenOption === 'function' ? this.tokenOption() : this.tokenOption;
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
    headers.set('X-Fleet-Git-Token', token);
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
    const orgId = typeof this.orgIdOption === 'function' ? this.orgIdOption() : this.orgIdOption;
    if (orgId?.trim()) headers.set('X-Fleet-Org', orgId.trim());
    const method = (init.method ?? 'GET').toUpperCase();
    if (!['GET', 'HEAD', 'OPTIONS'].includes(method)) {
      if (!headers.has('X-CSRF-Token')) {
        const csrf =
          typeof this.csrfTokenOption === 'function'
            ? this.csrfTokenOption()
            : this.csrfTokenOption;
        if (csrf?.trim()) headers.set('X-CSRF-Token', csrf.trim());
      }
      if (options.idempotencyKey?.trim() && !headers.has('Idempotency-Key')) {
        headers.set('Idempotency-Key', options.idempotencyKey.trim());
      }
    }

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
        credentials: 'same-origin',
      });
      if (!response.ok && response.status !== 304) {
        const body = await this.readJson(response);
        const error = errorFromResponse(response.status, body, requestId);
        if (response.status === 401) this.onUnauthorized?.(error);
        else if (response.status === 403) this.onForbidden?.(error);
        throw error;
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
