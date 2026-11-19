import { isSshRoute } from "./SshProxyFields";
import {
  sshBeginConnection,
  sshRespondHostKey,
  sshCancelHostKey,
  sshRespondKeyboardInteractive,
  sshCancelConnectionAttempt,
  type SshAuthenticationChallenge,
  type SshConnectionAttempt,
  type SshHostKeyChallenge,
  type SshHostKeyDecision,
} from "../../api";

export interface SshConfirmationState {
  challenge: SshHostKeyChallenge | null;
  authenticationChallenge: SshAuthenticationChallenge | null;
  busy: boolean;
}

type PendingChallenge =
  | { kind: "host_key"; value: SshHostKeyChallenge }
  | { kind: "authentication"; value: SshAuthenticationChallenge };

interface Services {
  begin: (id: string) => Promise<SshConnectionAttempt>;
  respond: typeof sshRespondHostKey;
  cancel: typeof sshCancelHostKey;
  respondAuthentication: typeof sshRespondKeyboardInteractive;
  cancelAuthentication: typeof sshCancelConnectionAttempt;
}

/** View-owned orchestration only. Backend remains the sole trust/expiry authority. */
export function createSshConnectionFlow(
  onState: (state: SshConfirmationState) => void,
  api: Services = {
    begin: sshBeginConnection,
    respond: sshRespondHostKey,
    cancel: sshCancelHostKey,
    respondAuthentication: sshRespondKeyboardInteractive,
    cancelAuthentication: sshCancelConnectionAttempt,
  },
  now: () => number = Date.now,
) {
  type Active = {
    id: string;
    attemptId: string | null;
    challenge: PendingChallenge | null;
    submitting: boolean;
    promise: Promise<boolean>;
    resolve: (connected: boolean) => void;
    reject: (error: unknown) => void;
  };
  let active: Active | null = null;
  let mounted = true;
  const publish = (state: SshConfirmationState) => { if (mounted) onState(state); };
  const clearState = () => publish({ challenge: null, authenticationChallenge: null, busy: false });
  const finish = (task: Active, success: boolean, failure?: unknown) => {
    if (active === task) { active = null; clearState(); }
    if (failure !== undefined) task.reject(failure); else task.resolve(success);
  };
  const cleanup = async (challenge: PendingChallenge) => {
    try {
      const { connection_id, attempt_id, challenge_id } = challenge.value;
      if (challenge.kind === "host_key") await api.cancel(connection_id, attempt_id, challenge_id);
      else await api.cancelAuthentication(connection_id, attempt_id, challenge_id);
    } catch (error) {
      console.warn("SSH challenge cleanup failed or was already invalidated", error);
    }
  };
  const cleanupOutcome = async (outcome: SshConnectionAttempt) => {
    if (outcome.status === "confirmation_required") {
      await cleanup({ kind: "host_key", value: outcome.challenge });
    } else if (outcome.status === "authentication_required") {
      await cleanup({ kind: "authentication", value: outcome.challenge });
    }
  };
  const abandon = (task: Active) => {
    finish(task, false);
    if (task.challenge && !task.submitting) void cleanup(task.challenge);
  };
  const acceptOutcome = async (task: Active, outcome: SshConnectionAttempt) => {
    if (active !== task || !mounted) {
      await cleanupOutcome(outcome);
      finish(task, false);
      return;
    }
    if (outcome.status === "connected") {
      finish(task, true);
      return;
    }

    const { connection_id, attempt_id } = outcome.challenge;
    if (connection_id !== task.id
      || (task.attemptId !== null && attempt_id !== task.attemptId)) {
      await cleanupOutcome(outcome);
      throw new Error("Unexpected SSH challenge identity");
    }
    task.attemptId = attempt_id;
    task.submitting = false;
    if (outcome.status === "confirmation_required") {
      const challenge = outcome.challenge;
      if (challenge.kind !== "unknown_host" || !["target", "jump"].includes(challenge.role) || !isSshRoute(challenge)
        || (challenge.role === "jump" && (challenge.route !== "jump" || challenge.host !== challenge.jump?.host
          || challenge.port !== challenge.jump.port || challenge.username !== challenge.jump.username))) {
        await cleanupOutcome(outcome);
        throw new Error("Unexpected SSH host-key challenge scope");
      }
      task.challenge = { kind: "host_key", value: challenge };
      publish({ challenge, authenticationChallenge: null, busy: false });
      return;
    }
    const challenge = outcome.challenge;
    if (challenge.role !== undefined && challenge.role !== "target" && challenge.role !== "jump") {
      await cleanupOutcome(outcome);
      throw new Error("Unexpected SSH authentication role");
    }
    task.challenge = { kind: "authentication", value: challenge };
    publish({ challenge: null, authenticationChallenge: challenge, busy: false });
  };

  return {
    activate() { mounted = true; clearState(); },
    dispose() { mounted = false; if (active) abandon(active); },
    connect(id: string): Promise<boolean> {
      if (!mounted) return Promise.resolve(false);
      if (active?.id === id) return active.promise;
      if (active) abandon(active);
      let resolve!: Active["resolve"];
      let reject!: Active["reject"];
      const promise = new Promise<boolean>((yes, no) => { resolve = yes; reject = no; });
      const task: Active = {
        id, attemptId: null, challenge: null, submitting: false, promise, resolve, reject,
      };
      active = task;
      void (async () => {
        try {
          await acceptOutcome(task, await api.begin(id));
        } catch (error) {
          finish(task, false, error);
        }
      })();
      return promise;
    },
    async respond(decision: SshHostKeyDecision) {
      const task = active;
      const challenge = task?.challenge;
      if (!task || challenge?.kind !== "host_key" || task.submitting || !mounted
        || now() >= challenge.value.expires_at_ms
        || (decision === "save" && !challenge.value.can_save)) return;
      task.submitting = true;
      publish({ challenge: challenge.value, authenticationChallenge: null, busy: true });
      try {
        const outcome = await api.respond(
          task.id,
          challenge.value.attempt_id,
          challenge.value.challenge_id,
          decision,
        );
        await acceptOutcome(task, outcome);
      } catch (error) {
        finish(task, false, error);
      }
    },
    async respondAuthentication(responses: string[]) {
      const task = active;
      const challenge = task?.challenge;
      if (!task || challenge?.kind !== "authentication" || task.submitting || !mounted
        || now() >= challenge.value.expires_at_ms) return;
      task.submitting = true;
      publish({ challenge: null, authenticationChallenge: challenge.value, busy: true });
      try {
        const outcome = await api.respondAuthentication(
          task.id,
          challenge.value.attempt_id,
          challenge.value.challenge_id,
          responses,
        );
        await acceptOutcome(task, outcome);
      } catch (error) {
        finish(task, false, error);
      }
    },
    async cancel() {
      const task = active;
      if (!task || task.submitting) return;
      const challenge = task.challenge;
      finish(task, false);
      if (challenge) await cleanup(challenge);
    },
  };
}
