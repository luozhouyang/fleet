import type { Attachment } from '@agent-fleet/protocol-types';
import { useQuery } from '@tanstack/react-query';
import { ArrowUp, Paperclip, Square } from 'lucide-react';
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ClipboardEvent,
  type DragEvent,
  type KeyboardEvent,
} from 'react';
import { Button } from '@/components/ui/button';
import { Spinner } from '@/components/ui/feedback';
import { apiClient } from '@/lib/api/client';
import { queryKeys } from '@/lib/query/keys';
import {
  SLASH_COMMANDS,
  applyMention,
  filterCommands,
  mentionQuery,
  mentionedPaths,
  moveItem,
  newQueuedPrompt,
  toWireAttachments,
  type PendingAttachment,
  type QueuedPrompt,
  type SlashCommand,
} from '@/lib/session/composer-model';
import { intakeAttachment, releaseAttachment } from '@/lib/session/attachment-intake';
import { getErrorMessage, isAbortError } from '@/lib/utils/format';
import { cn } from '@/lib/utils/cn';
import { AttachmentTray } from './attachment-tray';
import { CommandPicker } from './command-picker';
import { MentionPicker } from './mention-picker';
import { QueueList } from './queue-list';
import { useDraft, usePromptHistory } from './use-composer-state';

export interface ComposerSend {
  content: string;
  attachments: Attachment[];
}

/**
 * The session composer.
 *
 * Verbatim behavior worth keeping:
 * - Enter sends, Shift+Enter adds a newline, and an active IME composition
 *   never submits (the guard is on `isComposing`).
 * - The primary button becomes Stop while a turn is running, so stopping
 *   never requires leaving the keyboard.
 *
 * New in this version: persisted drafts, prompt recall with the arrow keys,
 * paste/drop attachments with client-side limits, a per-tab prompt queue and
 * `@file` / `/command` pickers.
 */
export function SessionComposer({
  sessionId,
  workspaceId,
  disabled,
  disabledReason,
  sending,
  running,
  interrupting,
  imagesSupported,
  provider,
  model,
  onSend,
  onInterrupt,
  onCommand,
}: {
  sessionId: string;
  workspaceId?: string;
  disabled: boolean;
  disabledReason?: string;
  sending: boolean;
  running: boolean;
  interrupting: boolean;
  imagesSupported: boolean;
  provider: string;
  model?: string;
  onSend: (message: ComposerSend) => Promise<void>;
  onInterrupt: () => void;
  onCommand: (command: SlashCommand) => void;
}) {
  const { draft, setDraft, clearDraft } = useDraft(sessionId);
  const history = usePromptHistory(sessionId);
  // While walking history, the arrows always mean older/newer prompt, even
  // though the caret sits at the end of the recalled text.
  const [recalling, setRecalling] = useState(false);
  const [attachments, setAttachments] = useState<PendingAttachment[]>([]);
  const [queue, setQueue] = useState<QueuedPrompt[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [intakeBusy, setIntakeBusy] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [caret, setCaret] = useState(0);
  const [pickerIndex, setPickerIndex] = useState(0);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Reset per-session state when the session changes.
  useEffect(() => {
    setAttachments((current) => {
      for (const attachment of current) releaseAttachment(attachment);
      return [];
    });
    setQueue([]);
    setError(null);
  }, [sessionId]);

  const mention = useMemo(() => mentionQuery(draft, caret), [draft, caret]);

  useEffect(() => {
    if (draft === '') setRecalling(false);
  }, [draft]);
  const commands = useMemo(() => filterCommands(draft), [draft]);

  const mentionPathsQuery = useQuery({
    queryKey: queryKeys.workspaceMentions(workspaceId ?? '', mention?.query ?? ''),
    queryFn: ({ signal }) =>
      apiClient.search(
        workspaceId ?? '',
        { query: mention?.query || '.', max_results: 40 },
        { signal },
      ),
    enabled: Boolean(workspaceId) && mention !== null && (mention?.query.length ?? 0) >= 1,
    staleTime: 10_000,
  });
  const mentionPaths = useMemo(() => {
    const seen = new Set<string>();
    const paths: string[] = [];
    for (const match of mentionPathsQuery.data?.matches ?? []) {
      if (seen.has(match.path)) continue;
      seen.add(match.path);
      paths.push(match.path);
      if (paths.length >= 8) break;
    }
    return paths;
  }, [mentionPathsQuery.data]);

  const pickerOpen = commands.length > 0 || (mention !== null && mentionPaths.length > 0);
  const pickerCount = commands.length > 0 ? commands.length : mentionPaths.length;
  const safeIndex = pickerCount === 0 ? 0 : Math.min(pickerIndex, pickerCount - 1);

  useEffect(() => {
    setPickerIndex(0);
  }, [draft]);

  const canSend =
    !disabled && !sending && !intakeBusy && Boolean(draft.trim() || attachments.length > 0);

  const addFiles = useCallback(
    async (files: FileList | File[]) => {
      const list = Array.from(files);
      if (list.length === 0) return;
      setError(null);
      setIntakeBusy(true);
      let remaining = attachments.length;
      const accepted: PendingAttachment[] = [];
      for (const file of list) {
        const result = await intakeAttachment(file, remaining, imagesSupported);
        if (!result.ok) {
          setError(result.error);
          continue;
        }
        accepted.push(result.attachment);
        remaining += 1;
      }
      setIntakeBusy(false);
      if (accepted.length) setAttachments((current) => [...current, ...accepted]);
    },
    [attachments.length, imagesSupported],
  );

  const removeAttachment = (id: string) => {
    setAttachments((current) => {
      const target = current.find((attachment) => attachment.id === id);
      if (target) releaseAttachment(target);
      return current.filter((attachment) => attachment.id !== id);
    });
  };

  const send = useCallback(async () => {
    const content = draft.trim();
    if (!content && attachments.length === 0) return;
    if (disabled || sending || intakeBusy) return;
    setError(null);
    const outgoing = attachments;
    const wire = toWireAttachments(outgoing);
    try {
      await onSend({ content, attachments: wire });
      clearDraft();
      for (const attachment of outgoing) releaseAttachment(attachment);
      setAttachments([]);
      if (content) history.record(content);
    } catch (sendError) {
      setError(
        isAbortError(sendError) ? 'The message request was cancelled.' : getErrorMessage(sendError),
      );
    }
  }, [attachments, clearDraft, disabled, draft, history, intakeBusy, onSend, sending]);

  // A queued prompt flushes as soon as the running turn ends.
  useEffect(() => {
    if (running || sending || queue.length === 0) return;
    const [first, ...rest] = queue;
    setQueue(rest);
    void (async () => {
      try {
        await onSend({
          content: first.content,
          attachments: toWireAttachments(first.attachments),
        });
        for (const attachment of first.attachments) releaseAttachment(attachment);
        if (first.content) history.record(first.content);
      } catch (queueError) {
        setError(getErrorMessage(queueError));
        // Put it back so nothing is silently lost.
        setQueue((current) => [first, ...current]);
      }
    })();
  }, [running, sending, queue, onSend, history]);

  const selectCommand = (command: SlashCommand) => {
    setDraft('');
    setError(null);
    onCommand(command);
  };

  const selectMention = (path: string) => {
    if (!mention) return;
    const applied = applyMention(draft, mention, path);
    setDraft(applied.value);
    setCaret(applied.caret);
    requestAnimationFrame(() => {
      const element = textareaRef.current;
      if (!element) return;
      element.focus();
      element.setSelectionRange(applied.caret, applied.caret);
    });
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    // Read the live caret: state can lag a programmatic value change, and the
    // recall shortcut must only fire when the caret is on the first line.
    const liveCaret = event.currentTarget.selectionStart ?? 0;
    // The picker owns the arrow keys while it is open.
    if (pickerOpen && pickerCount > 0) {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        const delta = event.key === 'ArrowDown' ? 1 : -1;
        setPickerIndex((index) => (index + delta + pickerCount) % pickerCount);
        return;
      }
      if (event.key === 'Enter' || event.key === 'Tab') {
        event.preventDefault();
        if (commands.length > 0) selectCommand(commands[safeIndex]);
        else if (mentionPaths[safeIndex]) selectMention(mentionPaths[safeIndex]);
        return;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        setDraft(draft.replace(/@[^\s]*$/, ''));
        return;
      }
    }

    // Up/down recall previous prompts when the caret sits on the first or
    // last line; anywhere else the arrows move within the draft.
    if (event.key === 'ArrowUp' && !event.shiftKey && (recalling || liveCaret === 0)) {
      const previous = history.previous();
      if (previous !== null) {
        event.preventDefault();
        setRecalling(true);
        setDraft(previous);
        setCaret(previous.length);
        requestAnimationFrame(() => {
          const element = textareaRef.current;
          element?.setSelectionRange(previous.length, previous.length);
        });
        return;
      }
    }
    if (event.key === 'ArrowDown' && !event.shiftKey && (recalling || liveCaret === draft.length)) {
      const next = history.next();
      if (next !== null) {
        event.preventDefault();
        setRecalling(next !== '');
        setDraft(next);
        setCaret(next.length);
        return;
      }
    }

    if (event.key === 'Enter' && !event.shiftKey) {
      if (event.nativeEvent.isComposing) return;
      event.preventDefault();
      if (running) {
        // Queue instead of failing: the backend rejects a second in-flight turn.
        if (canSend) {
          const content = draft.trim();
          setQueue((current) => [...current, newQueuedPrompt(content, attachments)]);
          clearDraft();
          setAttachments([]);
        }
        return;
      }
      void send();
    }
  };

  const handlePaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const files = Array.from(event.clipboardData?.files ?? []);
    if (files.length === 0) return;
    event.preventDefault();
    void addFiles(files);
  };

  const handleDrop = (event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setDragging(false);
    if (event.dataTransfer?.files?.length) void addFiles(event.dataTransfer.files);
  };

  const mentions = useMemo(() => mentionedPaths(draft), [draft]);

  return (
    <div className="relative">
      <QueueList
        queue={queue}
        onMove={(id, direction) =>
          setQueue((current) => {
            const index = current.findIndex((item) => item.id === id);
            return moveItem(current, index, index + direction);
          })
        }
        onRemove={(id) => setQueue((current) => current.filter((item) => item.id !== id))}
      />

      <CommandPicker commands={commands} activeIndex={safeIndex} onSelect={selectCommand} />
      {commands.length === 0 ? (
        <MentionPicker
          paths={mentionPaths}
          activeIndex={safeIndex}
          loading={mentionPathsQuery.isFetching}
          onSelect={selectMention}
        />
      ) : null}

      <div
        onDragOver={(event) => {
          event.preventDefault();
          setDragging(true);
        }}
        onDragLeave={() => setDragging(false)}
        onDrop={handleDrop}
        className={cn(
          'rounded-lg border bg-card p-2 shadow-card transition-[border-color,box-shadow] focus-within:border-ring focus-within:ring-2 focus-within:ring-ring/20',
          dragging ? 'border-primary ring-2 ring-primary/25' : 'border-border',
        )}
      >
        <AttachmentTray attachments={attachments} onRemove={removeAttachment} />

        <textarea
          ref={textareaRef}
          aria-label="Message the agent"
          value={draft}
          onChange={(event) => {
            setRecalling(false);
            setDraft(event.target.value);
            setCaret(event.target.selectionStart ?? event.target.value.length);
          }}
          onSelect={(event) => setCaret((event.target as HTMLTextAreaElement).selectionStart ?? 0)}
          onKeyDown={handleKeyDown}
          onPaste={handlePaste}
          rows={2}
          disabled={disabled}
          placeholder={
            disabled
              ? (disabledReason ?? 'This session is not accepting messages.')
              : 'Message the agent…  @ for files, / for commands'
          }
          className="max-h-40 min-h-[48px] w-full resize-none bg-transparent px-1 py-1 text-[12.5px] leading-5 text-foreground outline-none placeholder:text-faint disabled:cursor-not-allowed disabled:opacity-60"
        />

        <div className="mt-1 flex items-center justify-between gap-3 px-1">
          <div className="flex min-w-0 items-center gap-1.5 font-mono text-[9px] text-faint">
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="Attach a file"
              title="Attach a file"
              disabled={disabled || intakeBusy}
              onClick={() => fileInputRef.current?.click()}
            >
              <Paperclip className="size-3.5" />
            </Button>
            <input
              ref={fileInputRef}
              type="file"
              multiple
              hidden
              onChange={(event) => {
                if (event.target.files) void addFiles(event.target.files);
                event.target.value = '';
              }}
            />
            <span className="truncate">{provider}</span>
            {model ? (
              <>
                <span aria-hidden="true">·</span>
                <span className="truncate">{model}</span>
              </>
            ) : null}
            {mentions.length > 0 ? (
              <>
                <span aria-hidden="true">·</span>
                <span className="truncate" title={mentions.join(', ')}>
                  {mentions.length} file{mentions.length === 1 ? '' : 's'}
                </span>
              </>
            ) : null}
            <span className="hidden text-faint sm:inline" aria-hidden="true">
              ·
            </span>
            <span className="hidden shrink-0 sm:inline">Enter to send · ↑ recall</span>
          </div>
          <div className="flex shrink-0 items-center gap-1.5">
            {running ? (
              <Button
                variant="secondary"
                size="sm"
                aria-label="Stop the agent"
                disabled={interrupting}
                onClick={onInterrupt}
              >
                <Square className="size-3" aria-hidden="true" />
                {interrupting ? 'Stopping…' : 'Stop'}
              </Button>
            ) : null}
            <Button
              variant="primary"
              size="icon-sm"
              aria-label="Send message"
              title="Send message"
              disabled={!canSend}
              onClick={() => void send()}
            >
              {sending || intakeBusy ? (
                <Spinner className="size-3.5 text-primary-foreground" />
              ) : (
                <ArrowUp className="size-3.5" />
              )}
            </Button>
          </div>
        </div>
      </div>

      {!disabled ? (
        <p className="mt-1 px-1 text-[9.5px] text-faint">
          {SLASH_COMMANDS.length} commands · drag files or paste an image
        </p>
      ) : null}

      {error ? (
        <p className="mt-1.5 rounded-md border border-destructive/20 bg-destructive-soft px-2.5 py-1.5 text-[11px] text-destructive">
          {error}
        </p>
      ) : null}
    </div>
  );
}
