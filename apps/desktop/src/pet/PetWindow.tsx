import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../components/shadcn/button";
import { ContextMenu, ContextMenuTrigger } from "../components/shadcn/base-context-menu";
import { PetMenu } from "./PetMenu";
import { PetPlayer } from "./PetPlayer";
import { useIdleAction } from "./useIdleAction";
import { getPet, getWork, isPetAwake, listenPet, petDrag, petReady, renderFailed, setPetInteraction,
  workState, type Completion, type PetSnapshot, type Preferences, type Rect, type WorkState } from "./api";

function report(problem: unknown) {
  console.error("Desktop pet:", problem);
  void renderFailed(String(problem).slice(0, 400)).catch(console.error);
}

export function PetWindow() {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<PetSnapshot>();
  const [visible, setVisible] = useState(false);
  const [work, setWork] = useState<WorkState>("idle");
  const [completion, setCompletion] = useState<Completion | null>(null);
  const [playingClick, setPlayingClick] = useState(false);
  const [pointerHeld, setPointerHeld] = useState(false);
  const [manualAction, setManualAction] = useState<string>();
  const [variant, setVariant] = useState(0);
  const [menuOpen, setMenuOpen] = useState(false);
  const [menuBounds, setMenuBounds] = useState<Rect | null>(null);
  const ready = useRef(false);
  const body = useRef<HTMLButtonElement>(null);
  const bubble = useRef<HTMLDivElement>(null);
  const pointer = useRef<{ id: number; x: number; y: number; dragged: boolean } | null>(null);
  const completionTimer = useRef<number>();

  useEffect(() => {
    let live = true;
    let newest: Preferences | undefined;
    const listeners = [
      listenPet<Preferences>("preferences", preferences => {
        newest = preferences;
        if (live) setSnapshot(current => current && { ...current, preferences });
      }),
      listenPet<boolean>("visibility", value => { if (live) setVisible(value); }),
      listenPet<Rect | null>("menu-bounds", bounds => { if (live) setMenuBounds(bounds); }),
    ];
    void Promise.all(listeners).then(getPet).then(value => {
      if (live) {
        setSnapshot(newest ? { ...value, preferences: newest } : value);
        setVisible(current => current || value.visible);
      }
    }).catch(report);
    return () => {
      live = false;
      for (const listener of listeners) void listener.then(stop => stop(), () => undefined);
    };
  }, []);

  const prefs = snapshot?.preferences;
  const character = snapshot?.catalog.characters.find(item => item.id === prefs?.characterId);
  const awake = isPetAwake(prefs);
  const follow = Boolean(prefs?.followWork);
  useEffect(() => {
    if (!visible) { setWork("idle"); setCompletion(null); return; }
    if (!follow) setCompletion(null);
    let live = true;
    let pending = false;
    let failureReported = false;
    const poll = async () => {
      if (pending) return;
      pending = true;
      try {
        const next = await getWork();
        if (live) setWork(workState(next));
        failureReported = false;
      } catch (error) {
        if (live && !failureReported) { report(error); failureReported = true; }
      } finally { pending = false; }
    };
    const listener = follow ? listenPet<Completion>("completed", status => {
      if (!live) return;
      setCompletion(status);
      window.clearTimeout(completionTimer.current);
      completionTimer.current = window.setTimeout(() => setCompletion(null), 7000);
      void poll();
    }) : undefined;
    void listener?.catch(report);
    void poll();
    const timer = window.setInterval(() => void poll(), 1200);
    return () => {
      live = false;
      void listener?.then(stop => stop(), () => undefined);
      window.clearInterval(timer);
      window.clearTimeout(completionTimer.current);
    };
  }, [visible, follow]);

  const cue = !follow ? "idle" : work === "idle" && completion ? completion === "completed" ? "success" : "error" : work;
  const canIdle = awake && visible && work === "idle" && cue === "idle"
    && !manualAction && !playingClick && !pointerHeld && !menuOpen;
  const idle = useIdleAction(character, canIdle, report);
  const animation = manualAction ? "idle" : playingClick ? "click" : cue;
  const showBubble = prefs?.bubbles && !manualAction && !playingClick && (cue === "waiting" || cue === "success" || cue === "error");
  useLayoutEffect(() => {
    if (!character || !awake) return;
    const reportRegions = () => {
      if (!body.current) return;
      const regions = [body.current, bubble.current].flatMap(node => {
        if (!node) return [];
        const { x, y, width, height } = node.getBoundingClientRect();
        return [{ x, y, width, height }];
      });
      void setPetInteraction(regions, menuOpen).catch(report);
    };
    const observer = new ResizeObserver(reportRegions);
    if (body.current) observer.observe(body.current);
    if (bubble.current) observer.observe(bubble.current);
    observer.observe(document.documentElement);
    reportRegions();
    return () => observer.disconnect();
  }, [character, awake, menuOpen, showBubble, snapshot?.stageTop, snapshot?.stageLeft]);

  if (!snapshot || !character) return null;
  const hit = character.hitBox;
  const canvas = character.canvas;
  return <main className="pet-stage" style={{ top: snapshot.stageTop, left: snapshot.stageLeft, right: snapshot.stageLeft, aspectRatio: `${canvas.width}/${canvas.height}` }}>
    <PetPlayer key={character.id} character={character} root={snapshot.resourceRoot} state={animation} actionName={manualAction ?? idle.name} variant={variant} active={awake}
      onPlaying={idle.onPlaying}
      onReady={() => {
        if (!ready.current) { ready.current = true; void petReady().catch(error => { ready.current = false; report(error); }); }
      }} onEnded={() => {
        if (manualAction) setManualAction(undefined);
        else if (playingClick) setPlayingClick(false);
        else if (idle.name) idle.stop();
        else if (cue === "success" || cue === "error") setCompletion(null);
      }} onFailure={report} />
    {showBubble && <div ref={bubble} role="status" className="pet-bubble">{t(`desktopPet.${cue}`)}</div>}
    <ContextMenu onOpenChange={open => {
      if (open) idle.stop();
      setMenuOpen(open);
    }}>
      <ContextMenuTrigger render={
        <Button ref={body} aria-label={character.name} variant="ghost" className="pet-hit" style={{
          left: `${hit.x / canvas.width * 100}%`, top: `${hit.y / canvas.height * 100}%`,
          width: `${hit.width / canvas.width * 100}%`, height: `${hit.height / canvas.height * 100}%`,
        }} onPointerDown={event => {
          if (event.button !== 0 || !event.isPrimary) return;
          idle.stop();
          setPointerHeld(true);
          pointer.current = { id: event.pointerId, x: event.clientX, y: event.clientY, dragged: false };
          event.currentTarget.setPointerCapture(event.pointerId);
        }} onPointerMove={event => {
          const start = pointer.current;
          if (!start || start.id !== event.pointerId || start.dragged || !(event.buttons & 1)
              || Math.hypot(event.clientX - start.x, event.clientY - start.y) < 5) return;
          start.dragged = true;
          event.currentTarget.releasePointerCapture(event.pointerId);
          // The OS owns the drag; switching videos here can finish loading after release.
          void petDrag().catch(report).finally(() => {
            if (pointer.current === start || !pointer.current) setPointerHeld(false);
          });
        }} onPointerUp={event => {
          if (pointer.current?.id === event.pointerId && !pointer.current.dragged) setPointerHeld(false);
        }} onPointerCancel={event => {
          if (pointer.current?.id !== event.pointerId) return;
          if (!pointer.current.dragged) setPointerHeld(false);
          pointer.current = null;
        }} onClick={event => {
          const start = pointer.current;
          pointer.current = null;
          // A keyboard click has no pointer; a mouse click must come from a completed press.
          if (event.detail !== 0 && (!start || start.dragged)) return;
          idle.stop();
          setManualAction(undefined);
          setVariant(value => value + 1);
          setPlayingClick(true);
        }} />
      } />
      <PetMenu character={character} open={menuOpen} bounds={menuBounds} onError={report} onPlay={name => {
        idle.stop();
        setPlayingClick(false);
        setManualAction(name);
        setVariant(value => value + 1);
      }} />
    </ContextMenu>
  </main>;
}
