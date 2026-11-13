import { Button, Textarea, Spinner } from '@agent-fleet/ui';
import { useState, type KeyboardEvent } from 'react';
import { getErrorMessage } from '@/lib/utils/format';

export function SessionComposer({
  disabled,
  disabledReason,
  sending,
  onSend,
}: {
  disabled: boolean;
  disabledReason?: string;
  sending: boolean;
  onSend: (content: string) => Promise<void>;
}) {
  const [draft, setDraft] = useState('');
  const [error, setError] = useState<string | null>(null);
  const canSend = !disabled && !sending && Boolean(draft.trim());

  const submit = async () => {
    const content = draft.trim();
    if (!content || disabled || sending) return;
    setError(null);
    try {
      await onSend(content);
      setDraft('');
    } catch (sendError) {
      setError(getErrorMessage(sendError));
    }
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== 'Enter' || event.shiftKey) return;
    if (event.nativeEvent.isComposing) return;
    event.preventDefault();
    void submit();
  };

  return (
    <div className="shrink-0 rounded-xl border border-slate-800/90 bg-slate-900/70 p-3 shadow-lg shadow-slate-950/20">
      <Textarea
        aria-label="Message the coding agent"
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={handleKeyDown}
        placeholder={
          disabled
            ? (disabledReason ?? 'This session is not accepting messages.')
            : 'Describe the change, ask a question, or paste an error. Enter to send, Shift+Enter for a newline.'
        }
        disabled={disabled || sending}
        rows={3}
        className="max-h-52 min-h-20 border-slate-700/80 bg-slate-950/65"
      />
      <div className="mt-2 flex flex-wrap items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2 text-[11px] text-slate-600">
          {sending ? <Spinner size="sm" label="Sending message" /> : null}
          <span className="truncate">
            {disabled
              ? (disabledReason ?? 'Session closed')
              : 'Enter to send · Shift+Enter for a newline'}
          </span>
        </div>
        <Button variant="primary" size="sm" onClick={() => void submit()} disabled={!canSend}>
          Send message
        </Button>
      </div>
      {error ? (
        <p className="mt-2 rounded-md border border-red-500/25 bg-red-500/5 px-2.5 py-1.5 text-[11px] text-red-300">
          {error}
        </p>
      ) : null}
    </div>
  );
}
