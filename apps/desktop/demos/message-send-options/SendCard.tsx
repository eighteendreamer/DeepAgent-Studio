import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { ArrowUp, Check, CheckCheck, Plus, RotateCcw, Sparkles } from "lucide-react";
import { Button } from "../../src/components/shadcn/button";
import { graphemes, sendMotion, type Flight, type MotionProfile, type MotionSettings, type Variant } from "./sendMotion";

const SAMPLE = "把这个想法，顺着动作送进消息气泡。";
export type SendVariant = { id: Variant; name: string; description: string; tag: string };

type Message = { id: number; text: string };
export type Handle = { replay: () => void };

export const SendCard = forwardRef<Handle, { variant: SendVariant; motionProfile?: MotionProfile; selected: boolean; onSelect: () => void; slow: boolean; settings: MotionSettings }>(({ variant, motionProfile = "classic", selected, onSelect, slow, settings }, ref) => {
  const [draft, setDraft] = useState(SAMPLE);
  const [messages, setMessages] = useState<Message[]>([]);
  const [pending, setPending] = useState<number | null>(null);
  const [playing, setPlaying] = useState(false);
  const stageRef = useRef<HTMLDivElement>(null);
  const feedRef = useRef<HTMLDivElement>(null);
  const layerRef = useRef<HTMLDivElement>(null);
  const entryRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLTextAreaElement>(null);
  const mirrorRef = useRef<HTMLDivElement>(null);
  const flight = useRef<Flight>();
  const frame = useRef<number>();
  const counter = useRef(0);

  useEffect(() => {
    const finish = () => flight.current?.finish();
    window.addEventListener("resize", finish);
    return () => { window.removeEventListener("resize", finish); flight.current?.cancel(); window.cancelAnimationFrame(frame.current ?? 0); };
  }, []);

  const send = (replay = false) => {
    if (playing && !replay) return;
    if (replay) {
      flight.current?.cancel();
      flushSync(() => { setMessages([]); setPending(null); setPlaying(false); setDraft(SAMPLE); });
    }
    const text = replay ? SAMPLE : draft;
    const stage = stageRef.current, editor = editorRef.current, mirror = mirrorRef.current;
    if (!text.trim() || !stage || !editor || !mirror || !entryRef.current || !layerRef.current) return;
    const bounds = layerRef.current.getBoundingClientRect(), source = editor.getBoundingClientRect();
    const glyphs = Array.from(mirror.querySelectorAll<HTMLSpanElement>("span")).map(node => {
      const rect = node.getBoundingClientRect();
      return { text: node.textContent ?? "", x: rect.left - bounds.left,
        y: Math.max(source.top, Math.min(rect.top - editor.scrollTop, source.bottom - 20)) - bounds.top };
    });
    const id = ++counter.current;
    flushSync(() => {
      setDraft(""); setPlaying(true); setPending(id);
      setMessages(previous => [...previous.slice(-4), { id, text }]);
    });
    if (feedRef.current) feedRef.current.scrollTop = feedRef.current.scrollHeight;
    const target = feedRef.current?.querySelector<HTMLElement>(`[data-message="${id}"]`);
    if (!target) { setPlaying(false); return; }
    const rect = target.getBoundingClientRect();
    flight.current = sendMotion({ variant: variant.id, profile: motionProfile, layer: layerRef.current, composer: entryRef.current,
      start: { x: source.left - bounds.left - 12, y: source.top - bounds.top - 10, width: source.width + 24, height: Math.max(40, Math.min(source.height + 20, rect.height)) },
      end: { x: rect.left - bounds.left, y: rect.top - bounds.top, width: rect.width, height: rect.height },
      target, text, sourceGlyphs: glyphs, settings, slow,
      done: () => { flushSync(() => { setPending(null); setPlaying(false); }); },
    });
  };
  useImperativeHandle(ref, () => ({ replay: () => send(true) }));

  return <article className={`send-option ${selected ? "is-selected" : ""}`} data-option={variant.id}>
    <div className="option-heading">
      <Button variant="ghost" className="choose-option" aria-pressed={selected} aria-label={`选择方案 ${variant.id}：${variant.name}`} onClick={onSelect}>
        <span className="option-letter">{variant.id}</span><span>{variant.name}</span>{selected && <Check size={13}/>}
      </Button>
      <Button variant="ghost" size="icon" className="replay-option" aria-label={`重播方案 ${variant.id}`} onClick={() => send(true)}><RotateCcw size={13}/></Button>
    </div>
    <p className="option-description">{variant.description}</p>
    <div ref={stageRef} className="send-stage" data-playing={playing}>
      <div className="demo-chat-title"><span className="chat-status-dot"/>新对话<span>{variant.tag}</span></div>
      <div ref={feedRef} className="demo-feed" role="log" aria-label={`${variant.name}的消息记录`} aria-live="polite">
        <div className="assistant-message"><Sparkles size={16} className="assistant-mark"/><p>准备好了，发一条试试看。</p></div>
        {messages.map(message => <div className="user-message-row" key={message.id}>
          <div data-message={message.id} className="user-message" style={{ visibility: pending === message.id ? "hidden" : "visible" }}>
            {graphemes(message.text).map((character, index) => <span key={index} data-character>{character}</span>)}
          </div>
        </div>)}
      </div>
      <div className="entry-anchor">
        <div ref={entryRef} className="demo-composer">
          <div className="entry-text-wrap">
            <textarea ref={editorRef} aria-label={`方案 ${variant.id} 的消息`} value={draft} placeholder={playing ? "" : "写点什么，继续试试…"} rows={2} maxLength={180}
              onChange={event => setDraft(event.target.value)} onKeyDown={event => {
                if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); send(); }
              }}/>
            <div ref={mirrorRef} className="entry-mirror" aria-hidden="true">{graphemes(draft).map((character, index) => <span key={index}>{character}</span>)}</div>
          </div>
          <div className="entry-bottom"><span className="attachment-placeholder" aria-hidden="true"><Plus size={15}/></span><span className="entry-hint">Enter 发送</span>
            <Button variant="ghost" size="icon" className="demo-send-button" aria-label={`发送方案 ${variant.id} 的消息`} disabled={playing || !draft.trim()} onClick={() => send()}><ArrowUp size={15}/></Button>
          </div>
        </div>
      </div>
      <div ref={layerRef} className="send-flight-layer" aria-hidden="true"/>
    </div>
    <div className="option-footer"><span>{playing ? "正在送入气泡…" : "输入文字，点击箭头体验"}</span>
      <Button variant="ghost" className="select-text-button" onClick={onSelect} aria-label={`采用方案 ${variant.id}`}>{selected ? <><CheckCheck size={12}/>已选择</> : "选这个"}</Button>
    </div>
  </article>;
});
SendCard.displayName = "SendCard";
