import { memo } from 'react';
import { useSessionStore } from '@/lib/session-store';
import { QuestionTimeline } from '../question-timeline';

/**
 * A structured question inline in the transcript. The interactive answer
 * surface stays above the composer; this row is the transcript record.
 */
export const QuestionRow = memo(function QuestionRow({
  sessionId,
  questionId,
}: {
  sessionId: string;
  questionId: string;
}) {
  const question = useSessionStore((state) => state.sessions[sessionId]?.questions[questionId]);
  const answers = useSessionStore(
    (state) => state.sessions[sessionId]?.questionAnswers[questionId],
  );
  if (!question) return null;
  return (
    <div className="mb-4">
      <QuestionTimeline question={question} answers={answers} />
    </div>
  );
});
