import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { Terminal } from "@xterm/xterm";
import { Button } from "../shadcn/button";
import { Slider } from "../shadcn/slider";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";
import { applyCastEvent, type TerminalCast } from "./terminalRecording";

export function TerminalPlayback({ cast }: { cast: TerminalCast }) {
  const { t } = useTranslation(); const host = useRef<HTMLDivElement>(null); const term = useRef<Terminal | null>(null);
  const [ready, setReady] = useState(false), [playing, setPlaying] = useState(false), [position, setPosition] = useState(0), [speed, setSpeed] = useState("1");
  const cursor = useRef(0), time = useRef(0), generation = useRef(0), pending = useRef<Promise<void>>(Promise.resolve());
  const flush = () => new Promise<void>(resolve => { if (term.current) term.current.write("", resolve); else resolve(); });
  useEffect(() => {
    let disposed = false; let terminal: Terminal | undefined;
    void Promise.all([import("@xterm/xterm"), import("@xterm/xterm/css/xterm.css")]).then(([{ Terminal }]) => {
      if (disposed || !host.current) return;
      terminal = new Terminal({ cols: cast.width, rows: cast.height, disableStdin: true, scrollback: 3000, fontSize: 12, theme: { background: "#111827", foreground: "#e5e7eb" } });
      terminal.open(host.current); term.current = terminal; setReady(true);
    });
    return () => { disposed = true; generation.current++; term.current = null; terminal?.dispose(); };
  }, [cast]);
  const sink = () => ({ reset: () => term.current?.reset(), resize: (cols: number, rows: number) => term.current?.resize(cols, rows), write: (text: string) => term.current?.write(text) });
  async function seek(seconds: number) {
    setPlaying(false); const gen = ++generation.current;
    const operation = pending.current.then(async () => {
      await flush(); if (gen !== generation.current || !term.current) return;
      term.current.reset(); term.current.resize(cast.width, cast.height); cursor.current = 0;
      while (cursor.current < cast.events.length && cast.events[cursor.current][0] <= seconds) {
        if (gen !== generation.current || !term.current) return;
        for (let count = 0; count < 128 && cursor.current < cast.events.length && cast.events[cursor.current][0] <= seconds; count++) applyCastEvent(cast.events[cursor.current++], sink());
        await flush();
      }
      if (gen === generation.current) { time.current = seconds; setPosition(seconds); }
    });
    pending.current = operation.catch(() => {}); await operation;
  }
  useEffect(() => {
    if (!ready) return; void seek(0);
  }, [ready, cast]);
  useEffect(() => {
    if (!playing || !ready) return;
    let cancelled = false, frame = 0, previous = performance.now();
    const run = async (now: number) => {
      if (cancelled || !term.current) return;
      const target = Math.min(cast.duration, time.current + (now - previous) / 1000 * Number(speed)); previous = now;
      let count = 0;
      while (cursor.current < cast.events.length && cast.events[cursor.current][0] <= target && count++ < 256) applyCastEvent(cast.events[cursor.current++], sink());
      await flush(); if (cancelled) return;
      time.current = cursor.current < cast.events.length && cast.events[cursor.current][0] <= target ? cast.events[Math.max(0, cursor.current - 1)][0] : target;
      setPosition(time.current);
      if (time.current >= cast.duration && cursor.current >= cast.events.length) setPlaying(false); else frame = requestAnimationFrame(run);
    };
    frame = requestAnimationFrame(run); return () => { cancelled = true; cancelAnimationFrame(frame); };
  }, [playing, ready, speed, cast]);
  return <div className="space-y-3">
    <div className="max-h-[60vh] overflow-auto rounded-lg bg-gray-900 p-2"><div ref={host} /></div>
    <div className="flex items-center gap-3"><Button size="sm" disabled={!ready} onClick={() => setPlaying(value => !value)}>{t(`terminalWorkbench.${playing ? "pause" : "play"}`)}</Button>
      <Slider aria-label={t("terminalWorkbench.seek")} className="flex-1" min={0} max={Math.max(cast.duration, .001)} step={.01} value={[position]} onValueChange={values => void seek(values[0])} />
      <span className="text-xs tabular-nums">{position.toFixed(1)} / {cast.duration.toFixed(1)}s</span>
      <Select value={speed} onValueChange={setSpeed}><SelectTrigger className="h-8 w-20"><SelectValue /></SelectTrigger><SelectContent>{["0.5", "1", "2", "4"].map(value => <SelectItem key={value} value={value}>{value}×</SelectItem>)}</SelectContent></Select>
    </div>
  </div>;
}
