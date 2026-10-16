import type { QuestionRequest } from "./types";

export type QuestionQueues = ReadonlyMap<string, readonly QuestionRequest[]>;

/** Add a question to its owning session's FIFO queue without mutating state. */
export function enqueueQuestion(
  queues: QuestionQueues,
  request: QuestionRequest,
): Map<string, readonly QuestionRequest[]> {
  const queue = queues.get(request.session_id) ?? [];
  if (queue.some((item) => item.question_id === request.question_id)) {
    return new Map(queues);
  }
  const next = new Map(queues);
  next.set(request.session_id, [...queue, request]);
  return next;
}

/** Remove one question and delete the empty session queue. */
export function removeQuestion(
  queues: QuestionQueues,
  sessionId: string,
  questionId: string,
): Map<string, readonly QuestionRequest[]> {
  const queue = queues.get(sessionId);
  if (!queue) return new Map(queues);
  const remaining = queue.filter((item) => item.question_id !== questionId);
  const next = new Map(queues);
  if (remaining.length > 0) next.set(sessionId, remaining);
  else next.delete(sessionId);
  return next;
}

/** Clear only one session's pending questions. */
export function clearQuestionSession(
  queues: QuestionQueues,
  sessionId: string,
): Map<string, readonly QuestionRequest[]> {
  const next = new Map(queues);
  next.delete(sessionId);
  return next;
}

export function questionsForSession(
  queues: QuestionQueues,
  sessionId: string | null,
): readonly QuestionRequest[] {
  return sessionId ? queues.get(sessionId) ?? [] : [];
}
