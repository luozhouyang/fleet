import type {
  AgentQuestionAnswer,
  AgentQuestionRequest,
} from '@agent-fleet/protocol-types';
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import {
  initialQuestionDrafts,
  loadQuestionDraft,
  saveQuestionDraft,
} from '@/lib/question-draft';

export function QuestionComposer({
  question,
  pending,
  error,
  onSubmit,
  onReject,
}: {
  question: AgentQuestionRequest;
  pending: boolean;
  error?: string;
  onSubmit: (answers: AgentQuestionAnswer[]) => void;
  onReject: () => void;
}) {
  const [index, setIndex] = useState(0);
  const [drafts, setDrafts] = useState<Record<string, AgentQuestionAnswer>>(
    () => loadQuestionDraft(question) ?? initialQuestionDrafts(question),
  );
  const current = question.questions[index];
  if (!current) return null;

  const answer = drafts[current.id] ?? { id: current.id, selected: [] };
  const isLast = index >= question.questions.length - 1;

  const update = (next: AgentQuestionAnswer) => {
    const merged = { ...drafts, [current.id]: next };
    setDrafts(merged);
    saveQuestionDraft(question, merged);
  };

  const moveNext = () =>
    setIndex((value) => Math.min(value + 1, question.questions.length - 1));

  const toggle = (label: string) => {
    if (current.multi_select) {
      const selected = answer.selected.includes(label)
        ? answer.selected.filter((item) => item !== label)
        : [...answer.selected, label];
      update({ ...answer, selected });
      return;
    }
    update({ ...answer, selected: [label], custom: undefined });
    if (!isLast) moveNext();
  };

  const valid = answer.selected.length > 0 || Boolean(answer.custom?.trim());

  const submit = () =>
    onSubmit(
      question.questions.map(
        (item) => drafts[item.id] ?? { id: item.id, selected: [] },
      ),
    );

  const skip = () => {
    const cleared = {
      ...drafts,
      [current.id]: { id: current.id, selected: [] },
    };
    setDrafts(cleared);
    saveQuestionDraft(question, cleared);
    if (isLast) {
      onSubmit(
        question.questions.map(
          (item) => cleared[item.id] ?? { id: item.id, selected: [] },
        ),
      );
    } else {
      moveNext();
    }
  };

  return (
    <section
      data-testid="question-composer"
      className="rounded-xl border border-border bg-card p-4 shadow-card"
    >
      <header className="flex items-start justify-between gap-3">
        <div>
          <p className="font-mono text-[9px] uppercase tracking-[0.12em] text-faint">
            {current.header ?? 'Question'}
          </p>
          <p className="mt-0.5 text-[11px] text-faint">
            {index + 1} / {question.questions.length}
          </p>
        </div>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Close question"
          title="Reject question"
          onClick={onReject}
          disabled={pending}
        >
          ×
        </Button>
      </header>

      <p className="mt-3 text-[12.5px] leading-5 text-foreground">{current.question}</p>

      <div
        role={current.multi_select ? 'group' : 'radiogroup'}
        aria-label={current.question}
        className="mt-3 space-y-2"
      >
        {(current.options ?? []).map((option) => (
          <label
            key={option.label}
            className="flex cursor-pointer items-start gap-2 rounded-lg border border-border bg-background px-3 py-2"
          >
            <input
              type={current.multi_select ? 'checkbox' : 'radio'}
              name={current.id}
              checked={answer.selected.includes(option.label)}
              onChange={() => toggle(option.label)}
              disabled={pending}
              className="mt-1"
            />
            <span>
              <span className="block text-[12.5px] text-foreground">{option.label}</span>
              {option.description ? (
                <span className="block text-[10.5px] text-faint">
                  {option.description}
                </span>
              ) : null}
            </span>
          </label>
        ))}
      </div>

      {current.allow_custom ? (
        <input
          aria-label="Custom answer"
          type={current.is_secret ? 'password' : 'text'}
          placeholder="Type your answer…"
          value={answer.custom ?? ''}
          onChange={(event) =>
            update({ ...answer, custom: event.target.value })
          }
          disabled={pending}
          className="mt-3 w-full rounded-lg border border-border bg-background px-3 py-2 text-[12.5px] text-foreground outline-none placeholder:text-faint focus:border-ring focus:ring-2 focus:ring-ring/20"
        />
      ) : null}

      {error ? (
        <p className="mt-2 rounded-md border border-destructive/20 bg-destructive-soft px-2.5 py-1.5 text-[11px] text-destructive">
          {error}
        </p>
      ) : null}

      <footer className="mt-4 flex items-center justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={skip} disabled={pending}>
          Skip
        </Button>
        {index > 0 ? (
          <Button
            variant="secondary"
            size="sm"
            onClick={() => setIndex((value) => Math.max(0, value - 1))}
            disabled={pending}
          >
            Back
          </Button>
        ) : null}
        {!isLast ? (
          <Button
            variant="primary"
            size="sm"
            onClick={moveNext}
            disabled={!valid || pending}
          >
            Next
          </Button>
        ) : (
          <Button
            variant="primary"
            size="sm"
            onClick={submit}
            disabled={!valid || pending}
          >
            Submit
          </Button>
        )}
      </footer>
    </section>
  );
}
