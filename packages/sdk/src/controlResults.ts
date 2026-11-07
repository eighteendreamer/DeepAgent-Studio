/** Static contract details only. Invalid success does not mean no remote effect. */
export class HarnessResultError extends Error {
  constructor(readonly method: string, readonly field: string, readonly expected: string, readonly actual: string) {
    super(`${method} result has an invalid or conflicting ${field}: expected ${expected}, received ${actual}; remote outcome unknown, reconcile before retrying`);
    this.name = "HarnessResultError";
  }
}

type Schema = { ids?: string[]; status?: string; echo?: Array<[string, string]> };
const schemas: Record<string, Schema> = {
  "thread/start": { ids: ["threadId"], status: "ready" },
  "thread/resume": { ids: ["threadId"], status: "ready", echo: [["threadId", "threadId"]] },
  "thread/fork": { ids: ["threadId", "sourceThreadId"], echo: [["sourceThreadId", "threadId"]] },
  "thread/archive": { ids: ["threadId"], echo: [["threadId", "threadId"]] },
  "thread/unarchive": { ids: ["threadId"], echo: [["threadId", "threadId"]] },
  "thread/list": {},
  "turn/start": { ids: ["threadId", "turnId"], status: "started", echo: [["threadId", "threadId"]] },
  "turn/interrupt": { ids: ["threadId", "turnId"], status: "cancelling", echo: [["threadId", "threadId"], ["turnId", "turnId"]] },
  "turn/steer": { ids: ["threadId", "turnId", "replacesTurnId"], status: "started", echo: [["threadId", "threadId"], ["replacesTurnId", "turnId"]] },
  "approval/respond": { ids: ["approvalId"], status: "resolved", echo: [["approvalId", "approvalId"]] },
  "event/ack": { status: "accepted" }
};
const record = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const shape = (value: unknown): string => value === undefined ? "missing" : value === null ? "null" : Array.isArray(value) ? "array" : typeof value;
const unsigned = (value: unknown): boolean => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;

/** Minimal v1 mutation receipts; read-only projections keep their own contracts. */
export function validateControlResult(method: string, params: unknown, value: unknown): void {
  if (!Object.hasOwn(schemas, method)) return;
  if (method === "thread/list" && (!record(params) || params.archived === undefined)) return;
  const schema = schemas[method];
  const require = (valid: boolean, field: string, expected: string, actual: unknown): void => {
    if (!valid) throw new HarnessResultError(method, field, expected, shape(actual));
  };
  if (!record(value)) throw new HarnessResultError(method, "schema", "typed v1 mutation receipt", shape(value));
  const request = record(params) ? params : {};
  for (const field of schema.ids ?? []) {
    require(typeof value[field] === "string" && !!value[field].trim(), field, "nonblank identity", value[field]);
  }
  for (const [field, requested] of schema.echo ?? []) {
    require(value[field] === request[requested], field, "identity matching request", value[field]);
  }
  if (schema.status) require(value.status === schema.status, "status", schema.status, value.status);
  switch (method) {
    case "thread/list":
      require(Array.isArray(value.threads) && value.threads.every((row) => record(row) && row.archived === request.archived),
        "threads", "explicit active/archived view with matching markers", value.threads);
      break;
    case "thread/fork":
      require(unsigned(value.forkedAt), "forkedAt", "safe unsigned integer", value.forkedAt);
      require(Array.isArray(value.restoredPaths) && value.restoredPaths.every((path) => typeof path === "string"), "restoredPaths", "string array", value.restoredPaths);
      break;
    case "thread/archive":
      require(value.archived === true, "archived", "true", value.archived);
      break;
    case "thread/unarchive":
      require(value.archived === false, "archived", "false", value.archived);
      break;
    case "turn/steer":
      require(value.turnId !== request.turnId, "turnId", "new turn identity", value.turnId);
      break;
    case "approval/respond":
      require(typeof value.approved === "boolean" && value.approved === request.approved, "approved", "boolean matching request", value.approved);
      break;
    case "event/ack":
      require(unsigned(value.acknowledgedEventSequence) && value.acknowledgedEventSequence === request.eventSequence, "acknowledgedEventSequence", "safe unsigned integer matching request", value.acknowledgedEventSequence);
      break;
  }
}
