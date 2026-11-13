import {
  MessageRole,
  PermissionStatus,
  SessionStatus,
  type AgentMessage,
  type AgentPermissionRequest,
  type AgentSession,
} from '@agent-fleet/protocol-types';
import { Button, Card, EmptyState } from '@agent-fleet/ui';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useNavigate, useParams } from '@tanstack/react-router';
import { useEffect, useMemo, useRef, useState } from 'react';
import { ConfirmDialog } from '@/components/dialogs/ConfirmDialog';
import { ErrorState } from '@/components/feedback/ErrorState';
import { LoadingState } from '@/components/feedback/LoadingState';
import { MessageBubble } from '@/components/timeline/MessageBubble';
import { PermissionCard } from '@/components/timeline/PermissionCard';
import { ToolCallCard } from '@/components/timeline/ToolCallCard';
import { apiClient } from '@/lib/api/client';
import { useSessionDetail, useSessionPermissions } from '@/lib/query/hooks';
import { queryKeys } from '@/lib/query/keys';
import { buildTimeline } from '@/lib/session-timeline';
import { effectiveSessionStatus } from '@/lib/session-reducer';
import { useSessionStore } from '@/lib/session-store';
import { getErrorMessage } from '@/lib/utils/format';
import { SessionComposer } from './SessionComposer';
import { SessionHeader } from './SessionHeader';

const TERMINAL_STATUSES = new Set([
  SessionStatus.Completed,
  SessionStatus.Failed,
  SessionStatus.Terminated,
]);

function isAgentSession(value: unknown): value is AgentSession {
  return (
    typeof value === 'object' &&
    value !== null &&
    'id' in value &&
    'status' in value &&
    'provider' in value
  );
}

function isAgentMessage(value: unknown): value is AgentMessage {
  return (
    typeof value === 'object' &&
    value !== null &&
    'id' in value &&
    'role' in value &&
    'content' in value
  );
}

function localMessageId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return `local_${crypto.randomUUID()}`;
  }
  return `local_${Date.now().toString(36)}`;
}

export function SessionPage() {
  const params = useParams({ strict: false });
  const sessionId = typeof params.sessionId === 'string' ? params.sessionId : '';
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const sessionQuery = useSessionDetail(sessionId);
  const permissionsQuery = useSessionPermissions(sessionId);
  const view = useSessionStore((state) => state.sessions[sessionId]);
  const hydrateSession = useSessionStore((state) => state.hydrateSession);
  const mergePermissions = useSessionStore((state) => state.mergePermissions);
  const upsertSession = useSessionStore((state) => state.upsertSession);
  const addMessage = useSessionStore((state) => state.addMessage);
  const removeMessage = useSessionStore((state) => state.removeMessage);
  const setPermissionStatus = useSessionStore((state) => state.setPermissionStatus);

  const [pendingAction, setPendingAction] = useState<
    'interrupt' | 'resume' | 'close' | null
  >(null);
  const [closeDialogOpen, setCloseDialogOpen] = useState(false);
  const [resolvingPermissionId, setResolvingPermissionId] = useState<string | null>(null);
  const [pinned, setPinned] = useState(true);
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (sessionQuery.data) hydrateSession(sessionQuery.data);
  }, [sessionQuery.data, hydrateSession]);

  useEffect(() => {
    if (permissionsQuery.data) mergePermissions(permissionsQuery.data);
  }, [permissionsQuery.data, mergePermissions]);

  const actionMutation = useMutation({
    mutationFn: (kind: 'interrupt' | 'resume') =>
      kind === 'interrupt'
        ? apiClient.interruptSession(sessionId)
        : apiClient.resumeSession(sessionId),
    onSuccess: (result) => {
      if (isAgentSession(result)) upsertSession(result);
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.session(sessionId) });
    },
    onSettled: () => setPendingAction(null),
  });

  const closeMutation = useMutation({
    mutationFn: () => apiClient.deleteSession(sessionId),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.session(sessionId) });
      setCloseDialogOpen(false);
      void navigate({ to: '/sessions' });
    },
    onSettled: () => setPendingAction(null),
  });

  const permissionMutation = useMutation({
    mutationFn: ({
      permission,
      approve,
    }: {
      permission: AgentPermissionRequest;
      approve: boolean;
    }) =>
      approve
        ? apiClient.approvePermission(permission.id)
        : apiClient.rejectPermission(permission.id),
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

  const sendMutation = useMutation({
    mutationFn: (content: string) => apiClient.sendMessage(sessionId, { content }),
  });

  const session = view?.session ?? null;
  const status = view ? effectiveSessionStatus(view) : undefined;
  const entries = useMemo(() => (view ? buildTimeline(view) : []), [view]);

  useEffect(() => {
    if (!pinned) return;
    const element = scrollRef.current;
    if (!element) return;
    const frame = window.requestAnimationFrame(() => {
      element.scrollTop = element.scrollHeight;
    });
    return () => window.cancelAnimationFrame(frame);
  }, [entries, pinned]);

  if (sessionQuery.isLoading && !session) {
    return <LoadingState label="Loading session…" />;
  }
  if (sessionQuery.isError && !session) {
    return (
      <ErrorState
        error={sessionQuery.error}
        title="Could not load session"
        onRetry={() => void sessionQuery.refetch()}
      />
    );
  }
  if (!session && sessionQuery.data) {
    return <LoadingState label="Preparing session…" />;
  }
  if (!session || !status) {
    return (
      <Card>
        <EmptyState
          title="Session not found"
          description="This session may have been closed or removed."
          action={
            <Button variant="secondary" size="sm" onClick={() => void navigate({ to: '/sessions' })}>
              Back to sessions
            </Button>
          }
        />
      </Card>
    );
  }

  const terminal = TERMINAL_STATUSES.has(status);
  const sessionActive = !terminal;
  const disabledReason = terminal
    ? `Session is ${status.replaceAll('_', ' ')} and no longer accepts messages.`
    : undefined;
  const streamActive =
    status === SessionStatus.Running || status === SessionStatus.WaitingForInput;

  const jumpToLatest = () => {
    setPinned(true);
    const element = scrollRef.current;
    if (element) element.scrollTop = element.scrollHeight;
  };

  const handleSend = async (content: string) => {
    const optimistic: AgentMessage = {
      id: localMessageId(),
      session_id: sessionId,
      role: MessageRole.User,
      content,
      created_at: new Date().toISOString(),
    };
    addMessage(optimistic);
    try {
      const result = await sendMutation.mutateAsync(content);
      if (isAgentMessage(result)) {
        removeMessage(sessionId, optimistic.id);
        addMessage(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
    } catch (error) {
      removeMessage(sessionId, optimistic.id);
      throw error;
    }
  };

  const actionError = actionMutation.error
    ? getErrorMessage(actionMutation.error)
    : permissionMutation.error
      ? `Permission update failed: ${getErrorMessage(permissionMutation.error)}`
      : null;

  return (
    <div className="flex flex-col gap-3 lg:h-[calc(100dvh-7rem)] lg:min-h-[620px]">
      <SessionHeader
        session={session}
        status={status}
        lastError={view?.lastError}
        pendingAction={pendingAction}
        onInterrupt={() => {
          actionMutation.reset();
          setPendingAction('interrupt');
          actionMutation.mutate('interrupt');
        }}
        onResume={() => {
          actionMutation.reset();
          setPendingAction('resume');
          actionMutation.mutate('resume');
        }}
        onClose={() => {
          closeMutation.reset();
          setCloseDialogOpen(true);
        }}
      />

      {sessionQuery.isError ? (
        <div className="flex shrink-0 items-center justify-between gap-3 rounded-md border border-amber-500/20 bg-amber-500/5 px-3 py-2 text-[11px] text-amber-300/90">
          <span>Could not refresh session metadata. Live events are still applied.</span>
          <button
            type="button"
            onClick={() => void sessionQuery.refetch()}
            className="shrink-0 font-medium text-amber-200 hover:text-amber-100"
          >
            Retry
          </button>
        </div>
      ) : null}

      {actionError ? (
        <div className="shrink-0 rounded-md border border-red-500/25 bg-red-500/5 px-3 py-2 text-[11px] text-red-300">
          {actionError}
        </div>
      ) : null}

      <Card className="relative flex min-h-[46vh] flex-1 flex-col overflow-hidden lg:min-h-0">
        <div ref={scrollRef} onScroll={(event) => {
          const element = event.currentTarget;
          const distance = element.scrollHeight - element.scrollTop - element.clientHeight;
          setPinned(distance < 80);
        }} className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4">
          {entries.length === 0 ? (
            <EmptyState
              title="No messages yet"
              description="Send the first instruction below. Streaming responses, tool calls and permission requests will appear here."
            />
          ) : (
            entries.map((entry, index) => {
              if (entry.kind === 'message') {
                return (
                  <MessageBubble
                    key={`message-${entry.id}`}
                    message={entry.message}
                    streaming={
                      streamActive &&
                      index === entries.length - 1 &&
                      entry.message.role === MessageRole.Assistant
                    }
                  />
                );
              }
              if (entry.kind === 'tool') {
                return <ToolCallCard key={`tool-${entry.id}`} tool={entry.tool} />;
              }
              return (
                <PermissionCard
                  key={`permission-${entry.id}`}
                  permission={entry.permission}
                  sessionActive={sessionActive}
                  resolving={
                    permissionMutation.isPending &&
                    resolvingPermissionId === entry.permission.id
                  }
                  onResolve={(permission, approve) => {
                    permissionMutation.reset();
                    setResolvingPermissionId(permission.id);
                    permissionMutation.mutate({ permission, approve });
                  }}
                />
              );
            })
          )}
        </div>

        {!pinned ? (
          <div className="pointer-events-none absolute inset-x-0 bottom-3 flex justify-center">
            <Button
              variant="secondary"
              size="sm"
              onClick={jumpToLatest}
              className="pointer-events-auto shadow-lg shadow-black/30"
            >
              Jump to latest
            </Button>
          </div>
        ) : null}
      </Card>

      <SessionComposer
        disabled={terminal}
        disabledReason={disabledReason}
        sending={sendMutation.isPending}
        onSend={handleSend}
      />

      <ConfirmDialog
        open={closeDialogOpen}
        title="Close session?"
        description="The provider session will be shut down. This cannot be undone."
        confirmLabel="Close session"
        danger
        pending={closeMutation.isPending}
        error={closeMutation.error ? getErrorMessage(closeMutation.error) : undefined}
        onConfirm={() => {
          setPendingAction('close');
          closeMutation.mutate();
        }}
        onClose={() => {
          if (!closeMutation.isPending) {
            setCloseDialogOpen(false);
            setPendingAction(null);
            closeMutation.reset();
          }
        }}
      />
    </div>
  );
}

