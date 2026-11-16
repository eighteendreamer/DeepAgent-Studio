import { sshBeginConnection, sshRespondHostKey, sshCancelHostKey, type SshConnectionAttempt, type SshHostKeyChallenge, type SshHostKeyDecision } from "../../api";

export interface SshConfirmationState { challenge: SshHostKeyChallenge | null; busy: boolean }
interface Services {
  begin: (id: string) => Promise<SshConnectionAttempt>;
  respond: typeof sshRespondHostKey;
  cancel: typeof sshCancelHostKey;
}

/** View-owned orchestration only. Backend remains the sole trust/expiry authority. */
export function createSshConnectionFlow(
  onState: (state: SshConfirmationState) => void,
  api: Services = { begin: sshBeginConnection, respond: sshRespondHostKey, cancel: sshCancelHostKey },
  now: () => number = Date.now,
) {
  type Active = {
    id: string; challenge: SshHostKeyChallenge | null; submitting: boolean;
    promise: Promise<boolean>; resolve: (connected: boolean) => void; reject: (error: unknown) => void;
  };
  let active: Active | null = null;
  let mounted = true;
  const publish = (state: SshConfirmationState) => { if (mounted) onState(state); };
  const finish = (task: Active, success: boolean, failure?: unknown) => {
    if (active === task) { active = null; publish({ challenge: null, busy: false }); }
    if (failure !== undefined) task.reject(failure); else task.resolve(success);
  };
  const cleanup = async (challenge: SshHostKeyChallenge) => {
    try { await api.cancel(challenge.connection_id, challenge.attempt_id, challenge.challenge_id); }
    catch (error) { console.warn("SSH challenge cleanup failed or was already invalidated", error); }
  };
  const abandon = (task: Active) => {
    finish(task, false);
    if (task.challenge && !task.submitting) void cleanup(task.challenge);
  };
  return {
    activate() { mounted = true; publish({ challenge: null, busy: false }); },
    dispose() { mounted = false; if (active) abandon(active); },
    connect(id: string): Promise<boolean> {
      if (!mounted) return Promise.resolve(false);
      if (active?.id === id) return active.promise;
      if (active) abandon(active);
      let resolve!: Active["resolve"]; let reject!: Active["reject"];
      const promise = new Promise<boolean>((yes, no) => { resolve = yes; reject = no; });
      const task: Active = { id, challenge: null, submitting: false, promise, resolve, reject };
      active = task;
      void (async () => {
        try {
          const outcome = await api.begin(id);
          if (active !== task || !mounted) {
            if (outcome.status === "confirmation_required") await cleanup(outcome.challenge);
            finish(task, false); return;
          }
          if (outcome.status === "connected") { finish(task, true); return; }
          const challenge = outcome.challenge;
          if (challenge.connection_id !== id || challenge.kind !== "unknown_host" || challenge.role !== "target" || challenge.route !== "direct") {
            await cleanup(challenge);
            throw new Error("Unexpected SSH host-key challenge identity");
          }
          task.challenge = challenge;
          publish({ challenge, busy: false });
        } catch (error) { finish(task, false, error); }
      })();
      return promise;
    },
    async respond(decision: SshHostKeyDecision) {
      const task = active;
      const challenge = task?.challenge;
      if (!task || !challenge || task.submitting || !mounted || now() >= challenge.expires_at_ms || (decision === "save" && !challenge.can_save)) return;
      task.submitting = true;
      publish({ challenge, busy: true });
      try {
        await api.respond(task.id, challenge.attempt_id, challenge.challenge_id, decision);
        finish(task, mounted && active === task);
      } catch (error) { finish(task, false, error); }
    },
    async cancel() {
      const task = active;
      if (!task || task.submitting) return;
      finish(task, false);
      if (task.challenge) await cleanup(task.challenge);
    },
  };
}
