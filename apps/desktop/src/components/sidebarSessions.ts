import type { SessionSummary } from "../types";

export type SidebarSortCriterion = "updated" | "created";

/** A conversation has one sidebar owner: its project, or the no-project list. */
export function partitionSidebarSessions(
  sessions: SessionSummary[],
  sortCriterion: SidebarSortCriterion,
): { projectSessions: SessionSummary[]; recentSessions: SessionSummary[] } {
  const projectSessions: SessionSummary[] = [];
  const recentSessions: SessionSummary[] = [];
  for (const session of sessions) {
    (session.project ? projectSessions : recentSessions).push(session);
  }
  const sort = (items: SessionSummary[]) =>
    items.sort((a, b) =>
      Number(b.pinned) - Number(a.pinned) ||
      b[sortCriterion === "created" ? "created_at" : "updated_at"] -
        a[sortCriterion === "created" ? "created_at" : "updated_at"],
    );
  return { projectSessions: sort(projectSessions), recentSessions: sort(recentSessions) };
}
