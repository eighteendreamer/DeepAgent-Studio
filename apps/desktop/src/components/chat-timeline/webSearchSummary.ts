export const summarizeWebSearch = (output: Record<string, unknown>): string => {
  if (typeof output.error === "string") return output.error.slice(0, 200);
  const evidence = output.evidence && typeof output.evidence === "object"
    ? output.evidence as Record<string, unknown> : null;
  const count =
    typeof output.count === "number"
      ? output.count
      : Array.isArray(output.results)
      ? output.results.length
      : 0;
  const rawAttempts = evidence?.attempts ?? output.attempts;
  const rows = Array.isArray(rawAttempts)
    ? rawAttempts
        .map((item) => {
          if (!item || typeof item !== "object") return null;
          const attempt = item as Record<string, unknown>;
          const engine = evidence ? attempt.engine : attempt.provider;
          const name = typeof engine === "string" ? engine : null;
          if (!name) return null;
          return { name, ok: attempt.ok === true, count: attempt.result_count };
        })
        .filter((item): item is NonNullable<typeof item> => item !== null)
    : [];
  const provider = evidence
    ? rows.filter(item => item.ok && typeof item.count === "number" && item.count > 0)
      .map(item => item.name).join(" + ") || "configured engines"
    : typeof output.provider === "string" ? output.provider : "unknown";
  const attempts = rows.map(item => `${item.name}:${item.ok ? "ok" : "failed"}`).join(", ");
  const suffix = attempts ? `; attempts: ${attempts}` : "";
  const incomplete = evidence?.incomplete === true ? "; coverage incomplete" : "";
  return `web_search: ${provider} returned ${count} result(s)${suffix}${incomplete}`;
};
