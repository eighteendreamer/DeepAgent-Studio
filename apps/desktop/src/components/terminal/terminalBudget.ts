/** Process-local budgets shared by every mounted terminal workspace. */
export class TerminalBudget {
  private used = 0;
  constructor(readonly limit: number) {}
  reserve(bytes: number): boolean {
    if (!Number.isSafeInteger(bytes) || bytes < 0 || this.used + bytes > this.limit) return false;
    this.used += bytes; return true;
  }
  release(bytes: number) { this.used = Math.max(0, this.used - bytes); }
}
export const recordingBudget = new TerminalBudget(64 * 1024 * 1024);
export const receiveBudget = new TerminalBudget(32 * 1024 * 1024);
