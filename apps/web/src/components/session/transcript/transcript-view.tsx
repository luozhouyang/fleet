import type { AgentPermissionRequest } from '@agent-fleet/protocol-types';
import { useVirtualizer } from '@tanstack/react-virtual';
import { ArrowDown, MessageSquare } from 'lucide-react';
import { useEffect, useMemo, useRef, useState, type UIEvent } from 'react';
import { Button } from '@/components/ui/button';
import { EmptyState } from '@/components/ui/feedback';
import { useSessionStore } from '@/lib/session-store';
import {
  flattenTranscript,
  projectTranscript,
  type TranscriptRow,
} from '@/lib/session/transcript-model';
import { NoticeRow } from './notice-row';
import { MessageRow } from './message-row';
import { PermissionRow } from './permission-row';
import { PlanRow } from './plan-row';
import { QuestionRow } from './question-row';
import { ToolRow } from './tool-row';
import { TurnStatusRow } from './turn-status-row';

/** Below this many rows virtualization costs more than it saves. */
const VIRTUALIZE_THRESHOLD = 60;
const ROW_ESTIMATE = 84;

export function TranscriptView({
  sessionId,
  sessionActive,
  permissionBusy,
  resolvingPermissionId,
  onApprovePermission,
  onRejectPermission,
  pinToken,
}: {
  sessionId: string;
  sessionActive: boolean;
  permissionBusy: boolean;
  resolvingPermissionId: string | null;
  onApprovePermission: (permission: AgentPermissionRequest) => void;
  onRejectPermission: (permission: AgentPermissionRequest) => void;
  /** Bumped by the page when the user sends, to scroll back to the latest row. */
  pinToken: number;
}) {
  // Re-project only when the row set changes, not on every streaming delta.
  // Rows subscribe to their own records, so a delta re-renders one row while
  // this list stays untouched. The signature covers everything that changes
  // the set or order of rows: the order arrays, notice count and plan version.
  const signature = useSessionStore((state) => {
    const view = state.sessions[sessionId];
    if (!view) return '';
    return [
      view.messageOrder.join(','),
      view.toolOrder.join(','),
      view.permissionOrder.join(','),
      view.questionOrder.join(','),
      view.turnOrder.join(','),
      // Notices are capped, so the length stops changing once full; the id of
      // the newest notice keeps the signature moving after that.
      view.notices.length,
      view.notices.at(-1)?.id ?? '',
      view.plan ? `${view.plan.id}@${view.plan.updated_at}` : '',
    ].join('|');
  });
  const rows = useMemo<TranscriptRow[]>(() => {
    const view = useSessionStore.getState().sessions[sessionId];
    return view ? flattenTranscript(projectTranscript(view)) : [];
    // eslint-disable-next-line react-hooks/exhaustive-deps -- signature captures the projection inputs
  }, [sessionId, signature]);

  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const [pinned, setPinned] = useState(true);
  const pinnedRef = useRef(pinned);
  pinnedRef.current = pinned;
  const virtualize = rows.length >= VIRTUALIZE_THRESHOLD;

  const rowVirtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_ESTIMATE,
    overscan: 8,
    getItemKey: (index) => rows[index]?.key ?? index,
    enabled: virtualize,
  });
  const totalSize = virtualize ? rowVirtualizer.getTotalSize() : 0;

  // Keep the transcript anchored to the newest row while the user is at the
  // bottom. The measured total size is part of the dependency list because a
  // streaming row grows without changing the row count.
  useEffect(() => {
    if (!pinned) return;
    const element = scrollRef.current;
    if (!element) return;
    const frame = window.requestAnimationFrame(() => {
      element.scrollTop = element.scrollHeight;
    });
    return () => window.cancelAnimationFrame(frame);
  }, [pinned, rows, totalSize, pinToken]);

  useEffect(() => {
    setPinned(true);
  }, [pinToken]);

  // A row can grow without changing the signature (a streaming message, a
  // tool that finishes expanding). Follow the content while pinned so the
  // newest text stays visible even when the row set is unchanged.
  useEffect(() => {
    const content = contentRef.current;
    if (!content || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => {
      if (!pinnedRef.current) return;
      const element = scrollRef.current;
      if (element) element.scrollTop = element.scrollHeight;
    });
    observer.observe(content);
    return () => observer.disconnect();
  }, []);

  // The reducer knows exactly which assistant message is being written; a
  // finished message must render as markdown even while another turn (or a
  // trailing question) keeps the session in a running state.
  const streamingMessageId = useSessionStore(
    (state) => state.sessions[sessionId]?.streamingMessageId ?? null,
  );

  const handleScroll = (event: UIEvent<HTMLDivElement>) => {
    const element = event.currentTarget;
    const distance = element.scrollHeight - element.scrollTop - element.clientHeight;
    setPinned(distance < 80);
  };

  const jumpToLatest = () => {
    setPinned(true);
    const element = scrollRef.current;
    if (element) element.scrollTop = element.scrollHeight;
  };

  const renderRow = (row: TranscriptRow) => {
    switch (row.kind) {
      case 'message':
        return (
          <MessageRow
            sessionId={sessionId}
            messageId={row.message.id}
            streaming={streamingMessageId === row.message.id}
          />
        );
      case 'tool':
        return <ToolRow sessionId={sessionId} toolId={row.tool.id} />;
      case 'permission':
        return (
          <PermissionRow
            sessionId={sessionId}
            permissionId={row.permission.id}
            sessionActive={sessionActive}
            busy={permissionBusy}
            resolving={permissionBusy && resolvingPermissionId === row.permission.id}
            onApprove={onApprovePermission}
            onReject={onRejectPermission}
          />
        );
      case 'question':
        return <QuestionRow sessionId={sessionId} questionId={row.question.id} />;
      case 'notice':
        return <NoticeRow notice={row.notice} />;
      case 'plan':
        return <PlanRow plan={row.plan} />;
      case 'turn-status':
        return (
          <TurnStatusRow sessionId={sessionId} turnId={row.turn?.id ?? null} fallback={row.turn} />
        );
    }
  };

  return (
    <div className="relative min-h-0 flex-1">
      <div
        ref={scrollRef}
        onScroll={handleScroll}
        role="log"
        aria-label="Session transcript"
        className="h-full overflow-y-auto"
      >
        <div ref={contentRef} className="mx-auto w-full max-w-[700px] px-5 pb-6 pt-4">
          {rows.length === 0 ? (
            <EmptyState
              icon={<MessageSquare className="size-4" />}
              title="No messages yet"
              description="Send the first instruction below. Streaming responses, tool calls and permission requests will appear here."
            />
          ) : virtualize ? (
            <div style={{ height: totalSize, position: 'relative' }}>
              {rowVirtualizer.getVirtualItems().map((item) => (
                <div
                  key={item.key}
                  data-index={item.index}
                  ref={rowVirtualizer.measureElement}
                  className="absolute left-0 top-0 w-full"
                  style={{ transform: `translateY(${item.start}px)` }}
                >
                  {renderRow(rows[item.index])}
                </div>
              ))}
            </div>
          ) : (
            rows.map((row) => <div key={row.key}>{renderRow(row)}</div>)
          )}
        </div>
      </div>

      {!pinned && rows.length > 0 ? (
        <div className="pointer-events-none absolute inset-x-0 bottom-3 flex justify-center">
          <Button
            variant="secondary"
            size="sm"
            className="pointer-events-auto shadow-card"
            onClick={jumpToLatest}
          >
            <ArrowDown className="size-3.5" aria-hidden="true" />
            Jump to latest
          </Button>
        </div>
      ) : null}
    </div>
  );
}
