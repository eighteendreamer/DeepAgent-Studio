import { useEffect, useLayoutEffect, useRef, useState } from "react";
import gsap from "gsap";
import { Check, ChevronsUpDown, Link, LoaderCircle, Mail, Send, Share, X } from "lucide-react";
import { SHARE_MOTION, springEase } from "./share-motion";
import { Button } from "../../src/components/shadcn/button";
import { Input } from "../../src/components/shadcn/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../../src/components/shadcn/select";

type InviteRole = "Editor" | "Viewer";
type Person = { name: string; initials: string; role: InviteRole | "Owner"; email?: string; tone: string };
type Recipient = { email: string; role: InviteRole };
type SendState = "idle" | "sending" | "sent";

const INITIAL_PEOPLE: Person[] = [
  { name: "Ava Chen", initials: "AC", role: "Owner", tone: "sand" },
  { name: "Marco Rossi", initials: "MR", role: "Editor", tone: "sage" },
  { name: "Lena Park", initials: "LP", role: "Viewer", tone: "lilac" },
];
const EMAIL_PATTERN = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
const isEmail = (value: string) => EMAIL_PATTERN.test(value.trim());

function personFromRecipient({ email, role }: Recipient): Person {
  const localName = email.split("@")[0];
  const words = localName.split(/[._+-]+/).filter(Boolean);
  const name = words.map((word) => word[0].toUpperCase() + word.slice(1)).join(" ");
  const initials = words.length > 1 ? words.slice(0, 2).map((word) => word[0]).join("") : localName.slice(0, 2);
  return { name, initials: initials.toUpperCase(), role, email, tone: "wheat" };
}

export function ShareInvite() {
  const stageRef = useRef<HTMLElement>(null);
  const shellRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const compactRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const shareRef = useRef<HTMLButtonElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const actionRef = useRef<HTMLButtonElement>(null);
  const timelineRef = useRef<gsap.core.Timeline>();
  const resizeTweenRef = useRef<gsap.core.Tween>();
  const previousOpenRef = useRef<boolean>();
  const copyTimerRef = useRef<ReturnType<typeof setTimeout>>();
  const sendTimerRef = useRef<ReturnType<typeof setTimeout>>();
  const sendingRef = useRef(false);
  const previousCount = useRef(INITIAL_PEOPLE.length);
  const previousRecipientCount = useRef(0);
  const [open, setOpen] = useState(true);
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<InviteRole>("Editor");
  const [people, setPeople] = useState<Person[]>(INITIAL_PEOPLE);
  const [recipients, setRecipients] = useState<Recipient[]>([]);
  const [sendState, setSendState] = useState<SendState>("idle");
  const [copyState, setCopyState] = useState<"idle" | "copied" | "error">("idle");
  const [error, setError] = useState("");
  const [announcement, setAnnouncement] = useState("");
  const [reduceMotion, setReduceMotion] = useState(() => window.matchMedia("(prefers-reduced-motion: reduce)").matches);
  const canSend = recipients.length > 0 || isEmail(email);

  useEffect(() => {
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => setReduceMotion(query.matches);
    query.addEventListener("change", update);
    return () => {
      query.removeEventListener("change", update);
      timelineRef.current?.kill();
      resizeTweenRef.current?.kill();
      clearTimeout(copyTimerRef.current);
      clearTimeout(sendTimerRef.current);
    };
  }, []);

  // One sizing owner for both view changes and content changes. A second
  // layout tween used to overwrite the opening height's motion on every open.
  useLayoutEffect(() => {
    const shell = shellRef.current;
    const panel = panelRef.current;
    const compact = compactRef.current;
    if (!shell || !panel || !compact) return;
    const panelWidth = Math.min(320, window.innerWidth - 32);
    // offsetHeight measures layout; scrollHeight also includes the translated
    // entrance items and makes quick reversals accumulate extra card height.
    const targetHeight = open ? panel.offsetHeight + 2 : 56;
    const focusWasInside = shell.contains(document.activeElement);
    panel.toggleAttribute("inert", !open);
    compact.toggleAttribute("inert", open);
    const initialRender = previousOpenRef.current === undefined;
    const viewChanged = previousOpenRef.current !== open;
    previousOpenRef.current = open;

    if (initialRender || reduceMotion) {
      timelineRef.current?.kill();
      resizeTweenRef.current?.kill();
      gsap.set(shell, { width: open ? panelWidth : 200, height: targetHeight, borderRadius: open ? 32 : 30 });
      gsap.set(compact, { autoAlpha: open ? 0 : 1, scale: 1 });
      gsap.set(panel, { autoAlpha: open ? 1 : 0, filter: "blur(0px)" });
      gsap.set(panel.querySelectorAll("[data-reveal]"), { opacity: 1, y: 0, filter: "blur(0px)" });
      if (focusWasInside && viewChanged && !initialRender) (open ? closeRef : shareRef).current?.focus({ preventScroll: true });
      return;
    }

    if (!viewChanged) {
      // Retarget only height during a content update. An in-flight opening's
      // width and reveal sequence keep running instead of being restarted.
      resizeTweenRef.current?.kill();
      gsap.killTweensOf(shell, "height");
      const { stiffness, damping, duration } = SHARE_MOTION.resize;
      resizeTweenRef.current = gsap.to(shell, {
        height: targetHeight, duration, ease: springEase(stiffness, damping, duration),
      });
      return;
    }

    resizeTweenRef.current?.kill();
    timelineRef.current?.kill();
    const timeline = gsap.timeline();
    timelineRef.current = timeline;
    const motion = open ? SHARE_MOTION.open : SHARE_MOTION.close;
    timeline.to(shell, {
      width: open ? panelWidth : 200,
      duration: motion.width.duration,
      ease: springEase(motion.width.stiffness, motion.width.damping, motion.width.duration),
      overwrite: "auto",
    }, motion.width.delay);
    timeline.to(shell, {
      height: targetHeight,
      duration: motion.height.duration,
      ease: springEase(motion.height.stiffness, motion.height.damping, motion.height.duration),
      overwrite: "auto",
    }, motion.height.delay);
    timeline.to(shell, { borderRadius: open ? 32 : 30, duration: 0.22, ease: "power2.out" }, 0);
    timeline.to(compact, {
      autoAlpha: open ? 0 : 1,
      duration: 0.14,
      ease: "power2.out",
    }, open ? 0 : 0.2);
    if (!open) timeline.fromTo(compact, { scale: 0.95 }, {
      scale: 1, duration: 0.48, ease: springEase(300, 21, 0.48),
    }, 0.2);
    timeline.to(panel, {
      autoAlpha: open ? 1 : 0,
      filter: open ? "blur(0px)" : "blur(5px)",
      duration: open ? 0.24 : 0.18,
      ease: "power2.out",
    }, open ? 0.12 : 0);
    if (open) {
      timeline.fromTo(panel.querySelectorAll("[data-reveal]"),
        { opacity: 0, y: 10, filter: "blur(3px)" },
        { opacity: 1, y: 0, filter: "blur(0px)", duration: 0.5,
          stagger: 0.035, ease: springEase(280, 24, 0.5) }, 0.13);
    }
    if (focusWasInside) timeline.call(() => (open ? closeRef : shareRef).current?.focus({ preventScroll: true }), [], 0.36);
  }, [open, reduceMotion, recipients.length, people.length, error]);

  // Press feedback is confined to pointer-down/up; hover never moves a button.
  useEffect(() => {
    const stage = stageRef.current;
    if (!stage || reduceMotion) return;
    let pressed: HTMLElement | null = null;
    const tweens = new Map<HTMLElement, gsap.core.Tween>();
    const release = () => {
      if (!pressed) return;
      const button = pressed;
      pressed = null;
      tweens.get(button)?.kill();
      tweens.set(button, gsap.to(button, { scale: 1, duration: 0.42, ease: springEase(420, 24, 0.42) }));
    };
    const press = (event: PointerEvent) => {
      const button = (event.target as Element).closest<HTMLElement>(".role-trigger");
      if (!button || button.matches(":disabled")) return;
      release();
      pressed = button;
      tweens.get(button)?.kill();
      tweens.set(button, gsap.to(button, { scale: 0.965, duration: 0.08, ease: "power2.out" }));
    };
    stage.addEventListener("pointerdown", press);
    window.addEventListener("pointerup", release);
    window.addEventListener("pointercancel", release);
    return () => {
      stage.removeEventListener("pointerdown", press);
      window.removeEventListener("pointerup", release);
      window.removeEventListener("pointercancel", release);
      tweens.forEach((tween, button) => { tween.kill(); gsap.set(button, { clearProps: "transform" }); });
    };
  }, [reduceMotion]);

  // List entrances animate content only; shell sizing stays in the effect above.
  useLayoutEffect(() => {
    if (!open || !panelRef.current) return;
    const newRows = [...panelRef.current.querySelectorAll(".person-row")].slice(previousCount.current);
    previousCount.current = people.length;
    const entrance = newRows.length ? gsap.fromTo(newRows,
      { opacity: 0, y: -10, filter: "blur(4px)" },
      { opacity: 1, y: 0, filter: "blur(0px)", duration: reduceMotion ? 0 : 0.54, stagger: 0.05, ease: springEase(300, 24, 0.54) }) : null;
    const newChips = [...panelRef.current.querySelectorAll(".recipient-chip")].slice(previousRecipientCount.current);
    previousRecipientCount.current = recipients.length;
    const chipEntrance = newChips.length ? gsap.fromTo(newChips,
      { opacity: 0, y: -5, scale: 0.96, filter: "blur(2px)" },
      { opacity: 1, y: 0, scale: 1, filter: "blur(0px)", duration: reduceMotion ? 0 : 0.42, stagger: 0.04, ease: springEase(360, 24, 0.42) }) : null;
    return () => { entrance?.kill(); chipEntrance?.kill(); };
  }, [recipients, people, error, open, reduceMotion]);

  useEffect(() => {
    if (sendState !== "sending" || !actionRef.current || reduceMotion) return;
    const spinner = actionRef.current.querySelector(".sending-spinner");
    const tween = gsap.to(spinner, { rotation: 360, duration: 0.8, repeat: -1, ease: "none" });
    return () => { tween.kill(); };
  }, [sendState, reduceMotion]);

  function stageRecipient(): boolean {
    const value = email.trim().toLowerCase();
    if (!value) return true;
    if (!isEmail(value)) { setError("Enter a valid email address"); return false; }
    if (people.some((person) => person.email === value) || recipients.some((recipient) => recipient.email === value)) {
      setError("This person already has access or is in your invite list"); return false;
    }
    setRecipients((current) => [...current, { email: value, role }]);
    setEmail("");
    setError("");
    return true;
  }

  function sendInvite() {
    if (!canSend || sendingRef.current) return;
    const currentEmail = email.trim().toLowerCase();
    if (currentEmail && !stageRecipient()) return;
    const invites = [...recipients, ...(currentEmail ? [{ email: currentEmail, role }] : [])];
    if (!invites.length) return;
    sendingRef.current = true;
    setSendState("sending");
    setError("");
    sendTimerRef.current = setTimeout(() => {
      setPeople((current) => [...current, ...invites.map(personFromRecipient)]);
      setRecipients([]);
      setSendState("sent");
      setAnnouncement(`${invites.length === 1 ? "Invitation" : "Invitations"} added to this preview`);
      sendingRef.current = false;
      sendTimerRef.current = setTimeout(() => setSendState("idle"), 1400);
    }, reduceMotion ? 200 : 1050);
  }

  async function copyLink() {
    clearTimeout(copyTimerRef.current);
    try {
      const url = new URL(window.location.href);
      url.search = "";
      url.hash = "q4-plan";
      await navigator.clipboard.writeText(url.href);
      setCopyState("copied");
      setAnnouncement("Link copied to clipboard");
    } catch {
      setCopyState("error");
      setAnnouncement("Clipboard unavailable. Allow clipboard access and try again.");
    }
    copyTimerRef.current = setTimeout(() => setCopyState("idle"), 1800);
  }

  return (
    <main className="share-stage" ref={stageRef}>
      <div className="share-shell" ref={shellRef} data-open={open}>
        <div className="share-compact" ref={compactRef} aria-hidden={open}>
          <div className="avatar-stack" aria-label={`${people.length} people with access`}>
            {people.slice(0, 3).map((person) => <span key={person.name} className={`avatar avatar-${person.tone}`}>{person.initials}</span>)}
            {people.length > 3 && <span className="avatar avatar-extra">+{people.length - 3}</span>}
          </div>
          <Button className="share-button compact-share" ref={shareRef} onClick={() => setOpen(true)} aria-expanded={open} aria-controls="share-panel">
            <Share aria-hidden="true" /> <span>Share</span>
          </Button>
        </div>

        <div className="share-panel" id="share-panel" ref={panelRef} aria-hidden={!open}
          onKeyDown={(event) => { if (event.key === "Escape" && !event.defaultPrevented) setOpen(false); }}>
          <header className="share-header" data-reveal>
            <div className="title-pill"><Share aria-hidden="true" /><h1>Share “Q4 Plan”</h1></div>
            <Button className="share-button close-button" ref={closeRef} aria-label="Close share panel" onClick={() => setOpen(false)}><X aria-hidden="true" /></Button>
          </header>

          <form className="invite-form" onSubmit={(event) => { event.preventDefault(); if (sendState !== "sending") stageRecipient(); }} noValidate>
            <div className="invite-field" data-reveal>
              <Mail className="mail-icon" aria-hidden="true" />
              <Input ref={inputRef} className="email-input" type="email" aria-label="Add people by email" placeholder="Add people by email"
                value={email} disabled={sendState === "sending"} aria-invalid={Boolean(error)} aria-describedby={error ? "invite-error" : undefined}
                autoComplete="off" spellCheck={false}
                onChange={(event) => { setEmail(event.target.value); setError(""); if (sendState === "sent") setSendState("idle"); }}
                onKeyDown={(event) => {
                  if (event.key === "," || event.key === ";") { event.preventDefault(); stageRecipient(); }
                  if (event.key === "Escape") { event.stopPropagation(); setOpen(false); }
                }}
                onPaste={(event) => {
                  const values = event.clipboardData.getData("text").split(/[\s,;]+/).filter(Boolean);
                  if (values.length < 2) return;
                  event.preventDefault();
                  if (!values.every(isEmail)) { setError("Enter valid email addresses separated by commas"); return; }
                  const existing = new Set([...people.map((person) => person.email), ...recipients.map((recipient) => recipient.email)]);
                  const unique = [...new Set(values.map((value) => value.toLowerCase()))].filter((value) => !existing.has(value));
                  setRecipients((current) => [...current, ...unique.map((value) => ({ email: value, role }))]);
                  setEmail(""); setError("");
                }} />
              <Select value={role} onValueChange={(value) => setRole(value as InviteRole)}>
                <SelectTrigger className="role-trigger" aria-label="Invitation role" disabled={sendState === "sending"}>
                  <SelectValue /><ChevronsUpDown aria-hidden="true" className="role-chevrons" />
                </SelectTrigger>
                <SelectContent className="role-menu" align="end" sideOffset={6}>
                  <SelectItem className="role-option" value="Editor">Editor</SelectItem>
                  <SelectItem className="role-option" value="Viewer">Viewer</SelectItem>
                </SelectContent>
              </Select>
            </div>
            {recipients.length > 0 && <div className="recipient-chips" aria-label="Pending invitations">
              {recipients.map((recipient) => <div className="recipient-chip" key={recipient.email} title={`${recipient.email} · ${recipient.role}`}>
                <span className="avatar avatar-wheat chip-avatar">{`${recipient.email[0]}${recipient.email.split("@")[1][0]}`.toUpperCase()}</span>
                <span className="chip-email">{recipient.email}</span>
                <Button className="share-button chip-remove" aria-label={`Remove ${recipient.email}`} disabled={sendState === "sending"}
                  onClick={() => setRecipients((current) => current.filter((item) => item.email !== recipient.email))}><X aria-hidden="true" /></Button>
              </div>)}
            </div>}
            {error && <p className="invite-error" id="invite-error" role="alert">{error}</p>}
          </form>

          <section className="people-section" aria-labelledby="people-heading" data-reveal>
            <h2 id="people-heading">People with access</h2>
            <ul className="people-list">
              {people.map((person) => <li className="person-row" key={person.email ?? person.name}>
                <span className={`avatar avatar-${person.tone}`} aria-hidden="true">{person.initials}</span>
                <span className="person-name" title={person.email}>{person.name}</span>
                <span className="person-role">{person.role}</span>
              </li>)}
            </ul>
          </section>

          <footer className="share-actions" data-reveal>
            <Button className={`share-button copy-button ${copyState === "copied" ? "is-copied" : ""}`} onClick={copyLink}>
              {copyState === "copied" ? <Check aria-hidden="true" /> : <Link aria-hidden="true" />}
              <span>{copyState === "copied" ? "Copied" : copyState === "error" ? "Try again" : "Copy link"}</span>
            </Button>
            <Button className="share-button send-button" ref={actionRef} disabled={!canSend || sendState !== "idle"} data-state={sendState} onClick={sendInvite}>
              {sendState === "sending" ? <LoaderCircle className="sending-spinner" aria-hidden="true" /> : sendState === "sent" ? <Check aria-hidden="true" /> : <Send aria-hidden="true" />}
              <span>{sendState === "sending" ? "Sending" : sendState === "sent" ? "Invite sent" : "Send invite"}</span>
            </Button>
          </footer>
        </div>
      </div>
      <Button className="demo-signature" onClick={() => setOpen((value) => !value)} aria-label={open ? "Collapse share panel" : "Expand share panel"}>
        <span>{open ? "close" : "open"}</span><span aria-hidden="true">⟶</span><span aria-hidden="true">{`{}`}</span>
      </Button>
      <p className="sr-only" role="status" aria-live="polite">{announcement}</p>
    </main>
  );
}
