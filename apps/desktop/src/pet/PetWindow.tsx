import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../components/shadcn/button";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger } from "../components/shadcn/context-menu";
import { PetPlayer } from "./PetPlayer";
import { getPet, getWork, isPetAwake, listenPet, openMain, petDrag, petReady, renderFailed, resetPet, setPetInteraction,
  updatePet, workState, type Completion, type PetSnapshot, type Preferences, type WorkState } from "./api";

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
  const [gesture, setGesture] = useState<"click" | "drag" | null>(null);
  const [variant, setVariant] = useState(0);
  const [menuOpen, setMenuOpen] = useState(false);
  const ready = useRef(false);
  const body = useRef<HTMLButtonElement>(null);
  const bubble = useRef<HTMLDivElement>(null);
  const pointer = useRef<{ x: number; y: number; dragged: boolean } | null>(null);
  const gestureTimer = useRef<number>();
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
  const follow = visible && Boolean(prefs?.followWork);
  useEffect(() => {
    if (!follow) { setWork("idle"); setCompletion(null); return; }
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
    const listener = listenPet<Completion>("completed", status => {
      if (!live) return;
      setCompletion(status);
      window.clearTimeout(completionTimer.current);
      completionTimer.current = window.setTimeout(() => setCompletion(null), 7000);
      void poll();
    });
    void listener.catch(report);
    void poll();
    const timer = window.setInterval(() => void poll(), 1200);
    return () => {
      live = false;
      void listener.then(stop => stop(), () => undefined);
      window.clearInterval(timer);
      window.clearTimeout(completionTimer.current);
    };
  }, [follow]);

  const cue = work === "idle" && completion ? completion === "completed" ? "success" : "error" : work;
  const animation = gesture ?? cue;
  const showBubble = prefs?.bubbles && !gesture && (cue === "waiting" || cue === "success" || cue === "error");
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
  }, [character, awake, menuOpen, showBubble, snapshot?.stageTop]);
  useEffect(() => () => window.clearTimeout(gestureTimer.current), []);

  if (!snapshot || !character) return null;
  const hit = character.hitBox;
  const canvas = character.canvas;
  return <main className="pet-stage" style={{ top: snapshot.stageTop, aspectRatio: `${canvas.width}/${canvas.height}` }}>
    <PetPlayer key={character.id} character={character} root={snapshot.resourceRoot} state={animation} variant={variant} active={awake}
      onReady={() => {
        if (!ready.current) { ready.current = true; void petReady().catch(error => { ready.current = false; report(error); }); }
      }} onEnded={() => { setGesture(null); if (cue === "success" || cue === "error") setCompletion(null); }} onFailure={report} />
    {showBubble && <div ref={bubble} role="status" className="pet-bubble">{t(`desktopPet.${cue}`)}</div>}
    <ContextMenu modal={false} onOpenChange={setMenuOpen}>
      <ContextMenuTrigger asChild>
        <Button ref={body} aria-label={character.name} variant="ghost" className="pet-hit" style={{
          left: `${hit.x / canvas.width * 100}%`, top: `${hit.y / canvas.height * 100}%`,
          width: `${hit.width / canvas.width * 100}%`, height: `${hit.height / canvas.height * 100}%`,
        }} onPointerDown={event => {
          if (event.button !== 0) return;
          pointer.current = { x: event.clientX, y: event.clientY, dragged: false };
          event.currentTarget.setPointerCapture(event.pointerId);
        }} onPointerMove={event => {
          const start = pointer.current;
          if (!start || start.dragged || !(event.buttons & 1) || Math.hypot(event.clientX - start.x, event.clientY - start.y) < 5) return;
          start.dragged = true;
          event.currentTarget.releasePointerCapture(event.pointerId);
          setGesture("drag");
          window.clearTimeout(gestureTimer.current);
          gestureTimer.current = window.setTimeout(() => { pointer.current = null; setGesture(null); }, 1400);
          void petDrag().catch(error => { report(error); setGesture(null); });
        }} onPointerCancel={() => { pointer.current = null; }} onClick={() => {
          const dragged = pointer.current?.dragged;
          pointer.current = null;
          if (!dragged) { setVariant(value => value + 1); setGesture("click"); }
        }} />
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem onSelect={() => void openMain().catch(report)}>{t("desktopPet.openMain")}</ContextMenuItem>
        <ContextMenuItem onSelect={() => void updatePet({ sleeping: true }).catch(report)}>{t("desktopPet.sleep")}</ContextMenuItem>
        <ContextMenuItem onSelect={() => void resetPet().catch(report)}>{t("desktopPet.reset")}</ContextMenuItem>
        <ContextMenuItem disabled>{t("desktopPet.chat")}</ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  </main>;
}
