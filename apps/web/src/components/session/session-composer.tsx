import { ArrowUp } from 'lucide-react';
import { useState, type KeyboardEvent } from 'react';
import { Button } from '@/components/ui/button';
import { Spinner } from '@/components/ui/feedback';
import { getErrorMessage, isAbortError } from '@/lib/utils/format';

export function SessionComposer({
  disabled,
  disabledReason,
  sending,
  provider,
  model,
  onSend,
}: {
  disabled: boolean;
  disabledReason?: string;
  sending: boolean;
  provider: string;
  model?: string;
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
      if (isAbortError(sendError)) {
        setError('The message request was cancelled.');
      } else {
        setError(getErrorMessage(sendError));
      }
    }
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== 'Enter' || event.shiftKey) return;
    if (event.nativeEvent.isComposing) return;
    event.preventDefault();
    void submit();
  };

  return (
    <div>
      <div className="rounded-lg border border-border bg-card p-2 shadow-card transition-[border-color,box-shadow] focus-within:border-ring focus-within:ring-2 focus-within:ring-ring/20">
        <textarea
          aria-label="Message the agent"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={handleKeyDown}
          rows={2}
          disabled={disabled || sending}
          placeholder={
            disabled
              ? (disabledReason ?? 'This session is not accepting messages.')
              : 'Message the agent…'
          }
          className="max-h-40 min-h-[48px] w-full resize-none bg-transparent px-1 py-1 text-[12.5px] leading-5 text-foreground outline-none placeholder:text-faint disabled:cursor-not-allowed disabled:opacity-60"
        />
        <div className="mt-1 flex items-center justify-between gap-3 px-1">
          <div className="flex min-w-0 items-center gap-1.5 font-mono text-[9px] text-faint">
            <span className="truncate">{provider}</span>
            {model ? (
              <>
                <span aria-hidden="true">·</span>
                <span className="truncate">{model}</span>
              </>
            ) : null}
            <span className="hidden text-faint sm:inline" aria-hidden="true">
              ·
            </span>
            <span className="hidden shrink-0 sm:inline">Enter to send · Shift+Enter newline</span>
          </div>
          <Button
            variant="primary"
            size="icon-sm"
            aria-label="Send message"
            title="Send message"
            disabled={!canSend}
            onClick={() => void submit()}
          >
            {sending ? (
              <Spinner className="size-3.5 text-primary-foreground" />
            ) : (
              <ArrowUp className="size-3.5" />
            )}
          </Button>
        </div>
      </div>
      {error ? (
        <p className="mt-1.5 rounded-md border border-destructive/20 bg-destructive-soft px-2.5 py-1.5 text-[11px] text-destructive">
          {error}
        </p>
      ) : null}
    </div>
  );
}
