import {
  QuestionStatus,
  type AgentQuestionAnswer,
  type AgentQuestionRequest,
} from '@agent-fleet/protocol-types';
import { HelpCircle } from 'lucide-react';
import { StatusDot } from '@/components/ui/status-dot';
import { formatRelativeTime } from '@/lib/utils/format';
import { questionStatusMeta } from './session-utils';

export function QuestionTimeline({
  question,
  answers,
}: {
  question: AgentQuestionRequest;
  answers?: AgentQuestionAnswer[];
}) {
  const status = questionStatusMeta(question.status);
  const answerById = new Map((answers ?? []).map((answer) => [answer.id, answer]));
  const answered = question.status === QuestionStatus.Answered;
  const pending = question.status === QuestionStatus.Pending;

  return (
    <div
      data-testid="question-timeline"
      data-question-id={question.id}
      className="max-w-[600px] rounded-lg border border-border bg-card p-3 shadow-card-sm"
    >
      <div className="flex flex-wrap items-center gap-2">
        <HelpCircle className="size-3.5 shrink-0 text-primary" aria-hidden="true" />
        <span className="text-[12.5px] font-bold text-foreground">User question</span>
        <StatusDot tone={status.tone} size="sm" label={status.label} />
        {question.resolved_at ? (
          <span className="ml-auto shrink-0 font-mono text-[9px] text-faint">
            resolved {formatRelativeTime(question.resolved_at)}
          </span>
        ) : null}
      </div>

      <ul className="mt-2 flex flex-col gap-2.5">
        {question.questions.map((item) => {
          const answer = answerById.get(item.id);
          return (
            <li key={item.id}>
              {item.header ? (
                <p className="font-mono text-[9px] uppercase tracking-[0.12em] text-faint">
                  {item.header}
                </p>
              ) : null}
              <p className="whitespace-pre-wrap break-words text-[12px] leading-[18px] text-body">
                {item.question}
              </p>
              {answer ? (
                <p className="mt-1 flex flex-wrap items-center gap-1.5 text-[11.5px] leading-5">
                  {answer.selected.map((label) => (
                    <span
                      key={label}
                      className="rounded-md bg-inset px-1.5 py-0.5 font-mono text-[10.5px] text-foreground"
                    >
                      {label}
                    </span>
                  ))}
                  {answer.custom?.trim() ? (
                    <span className="whitespace-pre-wrap break-words text-body">
                      {answer.custom}
                    </span>
                  ) : null}
                </p>
              ) : null}
            </li>
          );
        })}
      </ul>

      {pending ? (
        <p className="mt-2 text-[10.5px] text-muted-foreground">
          Waiting for an answer.
        </p>
      ) : !answered ? (
        <p className="mt-2 text-[11px] text-muted-foreground">{status.label}.</p>
      ) : null}
    </div>
  );
}
