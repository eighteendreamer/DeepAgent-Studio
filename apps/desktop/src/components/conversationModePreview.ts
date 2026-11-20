export type GoalPreview = {
  runId: string;
  text: string;
  elapsedMs: number;
  resumedAt: number | null;
};

export type PlanPreview = { runId: string } & (
  | { phase: "drafting" | "planning" }
  | { phase: "review" | "executing"; markdown: string }
);

export type ConversationModePreview = {
  goal?: GoalPreview;
  plan?: PlanPreview;
};

export function goalElapsedMs(goal: GoalPreview, now: number): number {
  return goal.elapsedMs + (goal.resumedAt === null ? 0 : Math.max(0, now - goal.resumedAt));
}

export function toggleGoalPreview(goal: GoalPreview, now: number): GoalPreview {
  return goal.resumedAt === null
    ? { ...goal, resumedAt: now }
    : { ...goal, resumedAt: null, elapsedMs: goalElapsedMs(goal, now) };
}

export function settlePlanPreview(
  plan: PlanPreview | undefined,
  runId: string,
  succeeded: boolean,
  answer: string,
): PlanPreview | undefined {
  if (plan?.runId !== runId) return plan;
  if (plan.phase === "executing") return succeeded ? undefined : { ...plan, phase: "review" };
  if (plan.phase !== "drafting") return plan;
  return succeeded && answer.trim()
    ? { runId, phase: "review", markdown: answer }
    : { runId, phase: "planning" };
}

export function updateConversationModePreview(
  previews: ReadonlyMap<string, ConversationModePreview>,
  key: string,
  update: (current: ConversationModePreview) => ConversationModePreview,
): ReadonlyMap<string, ConversationModePreview> {
  const current = previews.get(key);
  const next = update(current ?? {});
  if (next === current || (!current && !next.goal && !next.plan)) return previews;
  const result = new Map(previews);
  if (next.goal || next.plan) result.set(key, next);
  else result.delete(key);
  return result;
}

export function adoptConversationModePreview(
  previews: ReadonlyMap<string, ConversationModePreview>,
  pendingKey: string,
  sessionId: string,
): ReadonlyMap<string, ConversationModePreview> {
  const preview = previews.get(pendingKey);
  if (!preview || pendingKey === sessionId) return previews;
  const result = new Map(previews);
  result.delete(pendingKey);
  result.set(sessionId, preview);
  return result;
}
