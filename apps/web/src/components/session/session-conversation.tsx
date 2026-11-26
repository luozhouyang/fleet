import {
  MessageRole,
  PermissionStatus,
  QuestionStatus,
  SessionStatus,
  type AgentMessage,
  type AgentPermissionRequest,
  type AgentQuestionAnswer,
  type AgentQuestionRequest,
} from '@agent-fleet/protocol-types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { Link, useNavigate } from '@tanstack/react-router';
import { Menu, PanelRightClose, PanelRightOpen, Square, Trash2 } from 'lucide-react';
import { useEffect, useState } from 'react';
import { ConfirmDialog } from '@/components/dialogs/confirm-dialog';
import { Button } from '@/components/ui/button';
import { ErrorState, LoadingState } from '@/components/ui/feedback';
import { StatusDot } from '@/components/ui/status-dot';
import { apiClient } from '@/lib/api/client';
import { newIdempotencyKey } from '@/lib/api/idempotency';
import {
  useMachines,
  useSessionDetail,
  useSessionPermissions,
  useSessionQuestions,
} from '@/lib/query/hooks';
import { queryKeys } from '@/lib/query/keys';
import { clearQuestionDraft } from '@/lib/question-draft';
import { useSessionStore } from '@/lib/session-store';
import { sessionIsActive, sessionStatusMeta } from '@/lib/status';
import { useEvent } from '@/lib/use-event';
import { formatRelativeTime, getErrorMessage, isAbortError, shortId } from '@/lib/utils/format';
import { machineDisplayName } from '@/lib/utils/machine';
import { ApprovalDialog } from './approval-dialog';
import { QuestionComposer } from './question-composer';
import { SessionComposer } from './session-composer';
import { TranscriptView } from './transcript/transcript-view';
import {
  isAgentMessage,
  isAgentSession,
  localMessageId,
  sessionHistoryUnavailable,
  sessionIsRestored,
  sessionModel,
  sessionTitle,
} from './session-utils';

const TERMINAL_STATUSES = new Set<SessionStatus>([
  SessionStatus.Interrupted,
  SessionStatus.Completed,
  SessionStatus.Failed,
  SessionStatus.Terminated,
]);

const RESUME_STATUSES = new Set<SessionStatus>([SessionStatus.Interrupted, SessionStatus.Failed]);

export function SessionConversation({
  sessionId,
  panelOpen,
  onTogglePanel,
  onOpenSidebar,
}: {
  sessionId: string;
  panelOpen: boolean;
  onTogglePanel: () => void;
  onOpenSidebar: () => void;
}) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const sessionQuery = useSessionDetail(sessionId);
  const permissionsQuery = useSessionPermissions(sessionId);
  const questionsQuery = useSessionQuestions(sessionId);
  const machinesQuery = useMachines();
  const liveSession = useSessionStore((state) => state.sessions[sessionId]?.session ?? null);
  const statusOverride = useSessionStore((state) => state.sessions[sessionId]?.statusOverride);
  const pendingQuestion = useSessionStore((state) => {
    const view = state.sessions[sessionId];
    if (!view) return undefined;
    for (const id of view.questionOrder) {
      const question = view.questions[id];
      if (question?.status === QuestionStatus.Pending) return question;
    }
    return undefined;
  });
  const hydrateSession = useSessionStore((state) => state.hydrateSession);
  const mergePermissions = useSessionStore((state) => state.mergePermissions);
  const mergeQuestions = useSessionStore((state) => state.mergeQuestions);
  const upsertSession = useSessionStore((state) => state.upsertSession);
  const addMessage = useSessionStore((state) => state.addMessage);
  const removeMessage = useSessionStore((state) => state.removeMessage);
  const setPermissionStatus = useSessionStore((state) => state.setPermissionStatus);
  const setQuestionStatus = useSessionStore((state) => state.setQuestionStatus);
  const setQuestionAnswers = useSessionStore((state) => state.setQuestionAnswers);

  const [pendingAction, setPendingAction] = useState<'interrupt' | 'resume' | 'close' | null>(null);
  const [closeDialogOpen, setCloseDialogOpen] = useState(false);
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);
  const [approvalPermission, setApprovalPermission] = useState<AgentPermissionRequest | null>(null);
  const [resolvingPermissionId, setResolvingPermissionId] = useState<string | null>(null);
  const [pinToken, setPinToken] = useState(0);
  const liveApproval = useSessionStore((state) =>
    approvalPermission ? state.sessions[sessionId]?.permissions[approvalPermission.id] : undefined,
  );

  useEffect(() => {
    if (sessionQuery.data) hydrateSession(sessionQuery.data);
  }, [sessionQuery.data, hydrateSession]);

  useEffect(() => {
    if (permissionsQuery.data) mergePermissions(permissionsQuery.data);
  }, [permissionsQuery.data, mergePermissions]);

  useEffect(() => {
    if (questionsQuery.data) mergeQuestions(questionsQuery.data);
  }, [questionsQuery.data, mergeQuestions]);

  useEffect(() => {
    setApprovalPermission(null);
  }, [sessionId]);

  // Stable identity: transcript permission rows keep their memoization while
  // the handler still sees the latest mutation state.
  const rejectPermission = useEvent((permission: AgentPermissionRequest) => {
    resolvePermission(permission, false);
  });

  const actionMutation = useMutation({
    mutationFn: ({
      kind,
      idempotencyKey,
    }: {
      kind: 'interrupt' | 'resume';
      idempotencyKey?: string;
    }) =>
      kind === 'interrupt'
        ? apiClient.interruptSession(sessionId, { idempotencyKey })
        : apiClient.resumeSession(sessionId),
    onSuccess: (result) => {
      if (isAgentSession(result)) upsertSession(result);
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.session(sessionId) });
      void queryClient.invalidateQueries({
        queryKey: queryKeys.sessionPermissions(sessionId),
      });
    },
    onSettled: () => setPendingAction(null),
  });

  const closeMutation = useMutation({
    mutationFn: (idempotencyKey: string) => apiClient.closeSession(sessionId, { idempotencyKey }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.session(sessionId) });
      void queryClient.invalidateQueries({
        queryKey: queryKeys.sessionPermissions(sessionId),
      });
      setCloseDialogOpen(false);
      void navigate({ to: '/sessions' });
    },
    onSettled: () => setPendingAction(null),
  });

  const deleteMutation = useMutation({
    mutationFn: () => apiClient.deleteSession(sessionId),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
      queryClient.removeQueries({ queryKey: queryKeys.session(sessionId) });
      queryClient.removeQueries({ queryKey: queryKeys.sessionPermissions(sessionId) });
      setDeleteDialogOpen(false);
      void navigate({ to: '/sessions' });
    },
  });

  const permissionMutation = useMutation({
    mutationFn: (variables: {
      permission: AgentPermissionRequest;
      approve: boolean;
      optionId?: string;
      idempotencyKey: string;
    }) => {
      const { permission, approve, optionId } = variables;
      const options = { idempotencyKey: variables.idempotencyKey };
      return approve
        ? apiClient.approvePermission(
            permission.session_id,
            permission.id,
            optionId,
            undefined,
            options,
          )
        : apiClient.rejectPermission(
            permission.session_id,
            permission.id,
            optionId,
            undefined,
            options,
          );
    },
    onSuccess: (_result, variables) => {
      setPermissionStatus(
        variables.permission.session_id,
        variables.permission.id,
        variables.approve ? PermissionStatus.Approved : PermissionStatus.Rejected,
      );
      void queryClient.invalidateQueries({ queryKey: queryKeys.session(sessionId) });
      void queryClient.invalidateQueries({
        queryKey: queryKeys.sessionPermissions(sessionId),
      });
    },
    onSettled: () => setResolvingPermissionId(null),
  });

  const answerQuestion = useMutation({
    mutationFn: ({
      question,
      answers,
      idempotencyKey,
    }: {
      question: AgentQuestionRequest;
      answers: AgentQuestionAnswer[];
      idempotencyKey: string;
    }) =>
      apiClient.answerQuestion(question.session_id, question.id, answers, {
        idempotencyKey,
      }),
    onSuccess: (_result, variables) => {
      clearQuestionDraft(variables.question);
      setQuestionStatus(
        variables.question.session_id,
        variables.question.id,
        QuestionStatus.Answered,
      );
      setQuestionAnswers(variables.question.session_id, variables.question.id, variables.answers);
      void queryClient.invalidateQueries({
        queryKey: queryKeys.sessionQuestions(sessionId),
      });
      void queryClient.invalidateQueries({ queryKey: queryKeys.session(sessionId) });
    },
  });

  const rejectQuestion = useMutation({
    mutationFn: ({
      question,
      idempotencyKey,
    }: {
      question: AgentQuestionRequest;
      idempotencyKey: string;
    }) =>
      apiClient.rejectQuestion(question.session_id, question.id, undefined, {
        idempotencyKey,
      }),
    onSuccess: (_result, variables) => {
      clearQuestionDraft(variables.question);
      setQuestionStatus(
        variables.question.session_id,
        variables.question.id,
        QuestionStatus.Rejected,
      );
      void queryClient.invalidateQueries({
        queryKey: queryKeys.sessionQuestions(sessionId),
      });
      void queryClient.invalidateQueries({ queryKey: queryKeys.session(sessionId) });
    },
  });

  const sendMutation = useMutation({
    mutationFn: ({ content, idempotencyKey }: { content: string; idempotencyKey: string }) =>
      apiClient.sendMessage(sessionId, { content }, { idempotencyKey }),
  });

  const session = liveSession ?? sessionQuery.data?.session ?? null;
  const status = statusOverride ?? session?.status;

  if (sessionQuery.isLoading && !session) {
    return (
      <div className="flex h-full min-h-0 flex-1 items-center justify-center">
        <LoadingState label="Loading session…" />
      </div>
    );
  }

  if (sessionQuery.isError && !session) {
    return (
      <div className="flex h-full min-h-0 flex-1 items-center justify-center px-6">
        <ErrorState
          title="Could not load session"
          message={getErrorMessage(sessionQuery.error)}
          onRetry={() => void sessionQuery.refetch()}
        />
      </div>
    );
  }

  if (!session || !status) {
    return (
      <div className="flex h-full min-h-0 flex-1 items-center justify-center">
        <LoadingState label="Preparing session…" />
      </div>
    );
  }

  const terminal = TERMINAL_STATUSES.has(status);
  const restored = sessionIsRestored(session);
  const historyUnavailable = sessionHistoryUnavailable(session);
  const readOnly = terminal || restored || historyUnavailable;
  const sessionActive = !terminal;
  const interruptible = sessionIsActive(status);
  const machine = machinesQuery.data?.find((candidate) => candidate.id === session.machine_id);
  const workspace = machine?.workspaces?.find((candidate) => candidate.id === session.workspace_id);
  const provider = machine?.providers?.find((candidate) => candidate.id === session.provider);
  const resumeSupported = provider?.capabilities.resume ?? true;
  const questionsUnsupported = provider?.capabilities.user_questions === false;
  const resumable = !restored && RESUME_STATUSES.has(status) && resumeSupported;
  const statusMeta = sessionStatusMeta(status);
  const streamingActive =
    status === SessionStatus.Running || status === SessionStatus.WaitingForInput;
  const machineName = machineDisplayName(machine);
  const workspaceName = session.workspace_id
    ? (workspace?.name ?? shortId(session.workspace_id, 12))
    : 'No workspace';
  const model = sessionModel(session);
  const busy =
    pendingAction !== null ||
    actionMutation.isPending ||
    closeMutation.isPending ||
    deleteMutation.isPending;
  const composerDisabledReason = historyUnavailable
    ? 'Session history is unavailable while the machine is offline.'
    : restored
      ? 'Restored after restart; start a new session.'
      : terminal
        ? `Session is ${status.replaceAll('_', ' ')} and no longer accepts messages.`
        : undefined;

  const actionError = actionMutation.error
    ? getErrorMessage(actionMutation.error)
    : permissionMutation.error && !approvalPermission
      ? `Permission update failed: ${getErrorMessage(permissionMutation.error)}`
      : null;
  const refreshError =
    sessionQuery.isError && !isAbortError(sessionQuery.error)
      ? 'Could not refresh session metadata. Live events are still applied.'
      : null;

  const approvalTarget = liveApproval ?? approvalPermission;
  const approvalOpen = approvalTarget?.status === PermissionStatus.Pending;

  const resolvePermission = (
    permission: AgentPermissionRequest,
    approve: boolean,
    optionId?: string,
    closeDialog = false,
  ) => {
    permissionMutation.reset();
    setResolvingPermissionId(permission.id);
    permissionMutation.mutate(
      {
        permission,
        approve,
        optionId,
        idempotencyKey: newIdempotencyKey('permission'),
      },
      {
        onSuccess: () => {
          if (closeDialog) setApprovalPermission(null);
        },
      },
    );
  };

  const handleSend = async (content: string) => {
    const optimistic: AgentMessage = {
      id: localMessageId(),
      session_id: sessionId,
      role: MessageRole.User,
      content,
      created_at: new Date().toISOString(),
    };
    setPinToken((token) => token + 1);
    addMessage(optimistic);
    try {
      const result = await sendMutation.mutateAsync({
        content,
        idempotencyKey: newIdempotencyKey('message'),
      });
      if (isAgentMessage(result)) {
        removeMessage(sessionId, optimistic.id);
        addMessage(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.session(sessionId) });
    } catch (error) {
      removeMessage(sessionId, optimistic.id);
      throw error;
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col bg-background">
      <header className="shrink-0 border-b border-border px-5 py-3">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0 flex-1">
            <div className="flex min-w-0 items-center gap-1.5">
              <Button
                variant="ghost"
                size="icon-sm"
                className="-ml-1.5 shrink-0 md:hidden"
                aria-label="Open session list"
                onClick={onOpenSidebar}
              >
                <Menu className="size-4" />
              </Button>
              <nav
                aria-label="Breadcrumb"
                className="flex min-w-0 items-center gap-1.5 font-mono text-[9px] uppercase tracking-[0.12em] text-faint"
              >
                <Link
                  to="/machines/$machineId"
                  params={{ machineId: session.machine_id }}
                  className="truncate transition-colors hover:text-foreground"
                >
                  {machineName}
                </Link>
                <span aria-hidden="true">/</span>
                {session.workspace_id ? (
                  <Link
                    to="/workspaces/$workspaceId"
                    params={{ workspaceId: session.workspace_id }}
                    className="truncate transition-colors hover:text-foreground"
                  >
                    {workspaceName}
                  </Link>
                ) : (
                  <span className="truncate">{workspaceName}</span>
                )}
                <span aria-hidden="true">/</span>
                <span className="truncate">{shortId(session.id, 12)}</span>
              </nav>
            </div>

            <div className="mt-1.5 flex min-w-0 flex-wrap items-center gap-2">
              <h1 className="min-w-0 truncate font-display text-[19px] font-bold leading-6 tracking-[-0.015em] text-foreground">
                {sessionTitle(session)}
              </h1>
              <StatusDot
                tone={statusMeta.tone}
                label={statusMeta.label}
                pulse={status === SessionStatus.Running}
              />
            </div>

            <p className="mt-1 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5 font-mono text-[9.5px] text-faint">
              <span>{session.provider}</span>
              <span aria-hidden="true">·</span>
              <span>{session.agent_type}</span>
              <span aria-hidden="true">·</span>
              <span>started {formatRelativeTime(session.created_at)}</span>
              {session.cwd ? (
                <>
                  <span aria-hidden="true">·</span>
                  <span className="max-w-full truncate" title={session.cwd}>
                    {session.cwd}
                  </span>
                </>
              ) : null}
            </p>
          </div>

          <div className="flex shrink-0 flex-wrap items-center gap-1.5">
            {interruptible ? (
              <Button
                variant="secondary"
                size="sm"
                disabled={busy}
                onClick={() => {
                  actionMutation.reset();
                  setPendingAction('interrupt');
                  actionMutation.mutate({
                    kind: 'interrupt',
                    idempotencyKey: newIdempotencyKey('session-interrupt'),
                  });
                }}
              >
                <Square className="size-3" aria-hidden="true" />
                {pendingAction === 'interrupt' ? 'Interrupting…' : 'Interrupt'}
              </Button>
            ) : null}
            {resumable ? (
              <Button
                variant="secondary"
                size="sm"
                disabled={busy}
                onClick={() => {
                  actionMutation.reset();
                  setPendingAction('resume');
                  actionMutation.mutate({ kind: 'resume' });
                }}
              >
                {pendingAction === 'resume' ? 'Resuming…' : 'Resume'}
              </Button>
            ) : null}
            <Button
              variant="ghost"
              size="sm"
              disabled={busy || status === SessionStatus.Terminated}
              onClick={() => {
                closeMutation.reset();
                setCloseDialogOpen(true);
              }}
            >
              Close session
            </Button>
            {terminal ? (
              <Button
                variant="ghost"
                size="sm"
                disabled={busy}
                className="text-destructive hover:bg-destructive-soft hover:text-destructive"
                onClick={() => {
                  deleteMutation.reset();
                  setDeleteDialogOpen(true);
                }}
              >
                <Trash2 className="size-3" aria-hidden="true" />
                Delete
              </Button>
            ) : null}
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={panelOpen ? 'Collapse context panel' : 'Expand context panel'}
              title={panelOpen ? 'Collapse context panel' : 'Expand context panel'}
              onClick={onTogglePanel}
            >
              {panelOpen ? (
                <PanelRightClose className="size-4" />
              ) : (
                <PanelRightOpen className="size-4" />
              )}
            </Button>
          </div>
        </div>
      </header>

      {historyUnavailable ? (
        <div className="shrink-0 border-b border-border bg-warning-soft px-5 py-2 text-[11px] text-warning">
          The machine is unreachable. The transcript lives on the daemon and cannot be loaded right
          now.
        </div>
      ) : restored ? (
        <div className="shrink-0 border-b border-border bg-warning-soft px-5 py-2 text-[11px] text-warning">
          Restored after a daemon restart. This history is read-only; start a new session to
          continue working.
        </div>
      ) : null}

      {actionError ? (
        <div className="shrink-0 border-b border-border bg-destructive-soft px-5 py-2 text-[11px] text-destructive">
          {actionError}
        </div>
      ) : refreshError ? (
        <div className="flex shrink-0 items-center justify-between gap-3 border-b border-border bg-warning-soft px-5 py-2 text-[11px] text-warning">
          <span>{refreshError}</span>
          <button
            type="button"
            onClick={() => void sessionQuery.refetch()}
            className="shrink-0 font-bold text-warning hover:underline"
          >
            Retry
          </button>
        </div>
      ) : null}

      <TranscriptView
        key={sessionId}
        sessionId={sessionId}
        streaming={streamingActive}
        sessionActive={sessionActive}
        permissionBusy={permissionMutation.isPending}
        resolvingPermissionId={resolvingPermissionId}
        onApprovePermission={setApprovalPermission}
        onRejectPermission={rejectPermission}
        pinToken={pinToken}
      />

      <div className="shrink-0 bg-background px-5 pb-4 pt-2">
        <div className="mx-auto w-full max-w-[640px]">
          {pendingQuestion ? (
            <QuestionComposer
              key={pendingQuestion.id}
              question={pendingQuestion}
              pending={answerQuestion.isPending || rejectQuestion.isPending}
              error={
                answerQuestion.error
                  ? getErrorMessage(answerQuestion.error)
                  : rejectQuestion.error
                    ? getErrorMessage(rejectQuestion.error)
                    : undefined
              }
              onSubmit={(answers) =>
                answerQuestion.mutate({
                  question: pendingQuestion,
                  answers,
                  idempotencyKey: newIdempotencyKey('question'),
                })
              }
              onReject={() =>
                rejectQuestion.mutate({
                  question: pendingQuestion,
                  idempotencyKey: newIdempotencyKey('question'),
                })
              }
            />
          ) : (
            <>
              <SessionComposer
                key={sessionId}
                disabled={readOnly}
                disabledReason={composerDisabledReason}
                sending={sendMutation.isPending}
                provider={session.provider}
                model={model}
                onSend={handleSend}
              />
              {questionsUnsupported ? (
                <p className="mt-1.5 text-[10px] text-faint">
                  This agent does not support structured questions; reply in chat.
                </p>
              ) : null}
            </>
          )}
        </div>
      </div>

      <ConfirmDialog
        open={closeDialogOpen}
        title="Close session?"
        description="The session will be archived and its history kept. You can permanently delete it later from the Closed filter."
        confirmLabel="Close session"
        danger
        pending={closeMutation.isPending}
        error={closeMutation.error ? getErrorMessage(closeMutation.error) : undefined}
        onConfirm={() => {
          setPendingAction('close');
          closeMutation.mutate(newIdempotencyKey('session-close'));
        }}
        onOpenChange={(open) => {
          if (!open && !closeMutation.isPending) {
            setCloseDialogOpen(false);
            closeMutation.reset();
          }
        }}
      />

      <ConfirmDialog
        open={deleteDialogOpen}
        title="Delete session permanently?"
        description="This removes the session and all of its recorded history. This cannot be undone."
        confirmLabel="Delete session"
        danger
        pending={deleteMutation.isPending}
        error={deleteMutation.error ? getErrorMessage(deleteMutation.error) : undefined}
        onConfirm={() => deleteMutation.mutate()}
        onOpenChange={(open) => {
          if (!open && !deleteMutation.isPending) {
            setDeleteDialogOpen(false);
            deleteMutation.reset();
          }
        }}
      />

      {approvalTarget && approvalOpen ? (
        <ApprovalDialog
          permission={approvalTarget}
          open
          pending={permissionMutation.isPending && resolvingPermissionId === approvalTarget.id}
          error={permissionMutation.error ? getErrorMessage(permissionMutation.error) : undefined}
          options={approvalTarget.options}
          onOpenChange={(open) => {
            if (!open && !permissionMutation.isPending) {
              permissionMutation.reset();
              setApprovalPermission(null);
            }
          }}
          onApprove={() => resolvePermission(approvalTarget, true, undefined, true)}
          onReject={() => resolvePermission(approvalTarget, false, undefined, true)}
          onResolve={(optionId, approve) =>
            resolvePermission(approvalTarget, approve, optionId, true)
          }
        />
      ) : null}
    </div>
  );
}
