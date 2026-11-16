import type {
  AgentQuestionAnswer,
  AgentQuestionRequest,
} from '@agent-fleet/protocol-types';

function draftKey(question: AgentQuestionRequest): string {
  return `fleet.question-draft.${question.session_id}.${question.id}`;
}

export function initialQuestionDrafts(
  question: AgentQuestionRequest,
): Record<string, AgentQuestionAnswer> {
  return Object.fromEntries(
    question.questions.map((item) => [item.id, { id: item.id, selected: [] }]),
  );
}

function hasSecretQuestion(question: AgentQuestionRequest): boolean {
  return question.questions.some((item) => item.is_secret);
}

export function loadQuestionDraft(
  question: AgentQuestionRequest,
): Record<string, AgentQuestionAnswer> | null {
  if (hasSecretQuestion(question)) return null;
  try {
    const raw = window.localStorage.getItem(draftKey(question));
    return raw ? (JSON.parse(raw) as Record<string, AgentQuestionAnswer>) : null;
  } catch {
    return null;
  }
}

export function saveQuestionDraft(
  question: AgentQuestionRequest,
  drafts: Record<string, AgentQuestionAnswer>,
): void {
  if (hasSecretQuestion(question)) return;
  try {
    window.localStorage.setItem(draftKey(question), JSON.stringify(drafts));
  } catch {
    // Draft persistence is best effort; the answer still submits.
  }
}

export function clearQuestionDraft(question: AgentQuestionRequest): void {
  try {
    window.localStorage.removeItem(draftKey(question));
  } catch {
    // Ignore storage failures.
  }
}
