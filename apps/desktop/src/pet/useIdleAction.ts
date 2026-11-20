import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { petRoam, type Character } from "./api";
import { chooseIdleAction, idleDelay, RUNNING_ACTION } from "./idle-actions";

type IdleAction = { name: string; moving: boolean };

export function useIdleAction(character: Character | undefined, eligible: boolean, report: (error: unknown) => void) {
  const [name, setName] = useState<string>();
  const current = useRef<IdleAction>();
  const previous = useRef<string>();

  const cancel = useCallback(() => {
    const active = current.current;
    current.current = undefined;
    if (active?.moving) void petRoam(null).catch(report);
  }, [report]);

  const stop = useCallback(() => {
    cancel();
    setName(undefined);
  }, [cancel]);

  useLayoutEffect(() => {
    if (!eligible) stop();
  }, [eligible, stop]);

  useLayoutEffect(() => {
    if (!eligible || !character || name) return;
    const timer = window.setTimeout(() => {
      const name = chooseIdleAction(character, previous.current);
      if (!name) return;
      const next = { name, moving: false };
      previous.current = name;
      current.current = next;
      setName(name);
    }, idleDelay());
    return () => window.clearTimeout(timer);
  }, [name, eligible, character]);

  useLayoutEffect(() => () => cancel(), [cancel]);

  const onPlaying = (video: HTMLVideoElement, actionName: string) => {
    const action = current.current;
    if (!eligible || !action || action.moving || action.name !== actionName || actionName !== RUNNING_ACTION) return;
    if (!Number.isFinite(video.duration)) { stop(); return; }
    action.moving = true;
    void petRoam({ preferRight: Math.random() < 0.5, durationMs: Math.round(video.duration * 1000) }).then(right => {
      if (current.current !== action) return;
      if (right === null) stop();
      else video.style.transform = right ? "scaleX(-1)" : "";
    }).catch(error => {
      if (current.current === action) stop();
      report(error);
    });
  };

  return { name: eligible ? name : undefined, onPlaying, stop };
}
