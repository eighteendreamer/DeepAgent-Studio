import { playMessageSendFlight, type SendBox, type SendFlight } from "./messageSendFlight";
import { motionColor, prefersReducedMotion } from "./springMotion";

type PendingSend = {
  text: string;
  id: string | null;
  taken: boolean;
  nextFrame: string | null;
  source: DOMRect;
  composer: HTMLElement;
  root: HTMLDivElement;
  carrier: HTMLDivElement;
  font: { fontSize: number; lineHeight: number };
  timeout: number;
  flight?: SendFlight;
  reveal?: () => void;
};

/** Visual state only; message identity comes from App's existing seeded turn.
 * Never matches a history bubble by its text or changes the send/run protocol. */
export class MessageSendMotion {
  private current: PendingSend | null = null;
  private targets = new Map<string, HTMLElement>();
  private frame: string | null = null;

  setFrame(frame: string) {
    if (this.frame === frame) return;
    this.frame = frame;
    if (this.current && this.current.nextFrame !== frame) this.finish();
  }

  prepare(editor: HTMLTextAreaElement, mirror: HTMLElement, composer: HTMLElement, text: string) {
    this.finish();
    if (prefersReducedMotion() || !text.trim()) return;
    const source = editor.getBoundingClientRect();
    if (!source.width || !source.height) return;
    const style = getComputedStyle(mirror);
    const root = document.createElement("div");
    root.className = "message-send-flight-root";
    root.setAttribute("aria-hidden", "true");
    const carrier = document.createElement("div");
    carrier.className = "message-send-carrier";
    Object.assign(carrier.style, {
      left: "0px", top: "0px", width: `${source.width}px`, height: `${source.height}px`,
      transform: `translate(${source.left}px, ${source.top}px)`, padding: "0px",
      fontFamily: style.fontFamily, fontSize: style.fontSize, lineHeight: style.lineHeight,
      fontWeight: style.fontWeight, letterSpacing: style.letterSpacing, color: style.color,
    });
    const content = mirror.cloneNode(true) as HTMLElement;
    content.removeAttribute("id");
    content.querySelectorAll(".composer-mirror-caret").forEach(node => node.remove());
    Object.assign(content.style, { position: "relative", inset: "auto", maxHeight: "none",
      minHeight: "0", height: "auto", overflow: "visible" });
    content.style.transform = `translateY(${-editor.scrollTop}px)`;
    carrier.append(content); root.append(carrier); document.body.append(root);
    const record: PendingSend = { text: text.trim(), id: null, taken: false, nextFrame: this.frame, source, composer, root, carrier,
      font: { fontSize: parseFloat(style.fontSize), lineHeight: parseFloat(style.lineHeight) },
      timeout: window.setTimeout(() => { if (this.current === record) this.finish(); }, 1800) };
    this.current = record;
  }

  /** Consume synchronously before App awaits preflight, then bind the actual id. */
  take(text: string) {
    const record = this.current;
    if (!record || record.taken || record.text !== text.trim()) return null;
    record.taken = true;
    return { bind: (id: string, nextFrame = this.frame) => {
      if (this.current !== record) return;
      record.id = id;
      record.nextFrame = nextFrame;
      // A cold, lazy-loaded ChatView may take longer than preflight. Keep the
      // source until that exact bubble mounts, without delaying the actual run.
      window.clearTimeout(record.timeout);
      record.timeout = window.setTimeout(() => { if (this.current === record) this.finish(); }, 5000);
      const target = this.targets.get(id);
      if (target) this.start(record, target);
    } };
  }

  register(id: string, target: HTMLElement) {
    this.targets.set(id, target);
    if (this.current?.id === id && !this.current.flight) this.start(this.current, target);
    return () => {
      if (this.targets.get(id) !== target) return;
      this.targets.delete(id);
      if (this.current?.id === id) this.finish();
    };
  }

  finish = () => {
    const record = this.current;
    if (!record) return;
    this.current = null;
    window.clearTimeout(record.timeout);
    record.flight?.cancel();
    record.reveal?.();
    record.root.remove();
    window.removeEventListener("resize", this.finish);
  };

  private start(record: PendingSend, target: HTMLElement) {
    const end = target.getBoundingClientRect();
    if (!end.width || !end.height || end.bottom < 0 || end.top > window.innerHeight || prefersReducedMotion()) {
      this.finish(); return;
    }
    const style = getComputedStyle(target);
    const paddingX = parseFloat(style.paddingLeft), paddingY = parseFloat(style.paddingTop);
    // Framer's layout projection writes inline visibility while the textarea
    // shrinks / timeline scrolls. A scoped CSS attribute must own this handoff.
    const previouslyHidden = target.hasAttribute("data-message-send-hidden");
    record.reveal = () => { if (!previouslyHidden) target.removeAttribute("data-message-send-hidden"); };
    target.setAttribute("data-message-send-hidden", "");
    record.carrier.replaceChildren(...Array.from(target.cloneNode(true).childNodes));
    Object.assign(record.carrier.style, {
      padding: style.padding, fontFamily: style.fontFamily, fontWeight: style.fontWeight,
      letterSpacing: style.letterSpacing, color: style.color, cornerShape: style.getPropertyValue("corner-shape"),
    });
    // Inline content retains its actual chips; attachments remain in their
    // existing message attachment row, including attachment-only submissions.
    const start: SendBox = { x: record.source.left-paddingX, y: record.source.top-paddingY,
      width: record.source.width+paddingX*2, height: Math.max(40, Math.min(record.source.height+paddingY*2, end.height)) };
    window.clearTimeout(record.timeout);
    window.addEventListener("resize", this.finish);
    record.flight = playMessageSendFlight({ carrier: record.carrier, composer: record.composer, start,
      getEnd: () => {
        if (!target.isConnected) return null;
        const rect = target.getBoundingClientRect();
        const scroll = target.closest<HTMLElement>("[data-chat-scroll]");
        const viewport = scroll?.getBoundingClientRect();
        if (viewport && (rect.bottom < viewport.top || rect.top > viewport.bottom)) return null;
        return { x: rect.left, y: rect.top, width: rect.width, height: rect.height };
      },
      appearance: { start: record.font, end: {
        fontSize: parseFloat(style.fontSize), lineHeight: parseFloat(style.lineHeight),
        background: motionColor(style.backgroundColor),
        radii: [style.borderTopLeftRadius, style.borderTopRightRadius, style.borderBottomRightRadius, style.borderBottomLeftRadius].map(value => parseFloat(value)),
      } }, onFinish: () => { if (this.current === record) this.finish(); },
    });
  }
}
