interface EntranceElements {
  overlay: HTMLElement;
  card: HTMLElement;
  welcome: HTMLElement;
  point: HTMLElement;
  doors: readonly HTMLElement[];
  seams: readonly HTMLElement[];
}

export interface OnboardingEntrance {
  finished: Promise<boolean>;
  cancel: () => void;
}

// Accepted preview: 2.4s text collapse → 3.4s point/line → 4.6s doors → 6.2s done.
const TIMING = { card: 600, welcomeIn: 1200, hold: 600, morph: 1000, line: 1200, doors: 1600 };
const EASE = "cubic-bezier(0.65, 0, 0.35, 1)";

/** Owns one entrance and all its browser animations, including cancellation. */
export function startOnboardingEntrance(
  elements: EntranceElements,
  reducedMotion: boolean,
): OnboardingEntrance {
  const { overlay, card, welcome, point, doors, seams } = elements;
  const tracks: Animation[] = [];
  let cancelled = false;
  const originalVisibility = welcome.style.visibility;
  const cancel = () => {
    cancelled = true;
    tracks.forEach(track => track.cancel());
    welcome.style.visibility = originalVisibility;
  };

  if (reducedMotion) {
    return { finished: Promise.resolve().then(() => !cancelled), cancel };
  }

  const animate = (
    element: HTMLElement,
    keyframes: Keyframe[],
    duration: number,
    delay = 0,
    easing = EASE,
  ) => {
    const track = element.animate(keyframes, { duration, delay, easing, fill: "forwards" });
    tracks.push(track);
    // Setup can throw before the aggregate promise exists; cancellation must
    // still observe every finished rejection. The aggregate reports failures.
    void track.finished.catch(() => {});
    return track;
  };

  try {
    const welcomeAt = TIMING.card;
    const morphAt = welcomeAt + TIMING.welcomeIn + TIMING.hold;
    const lineAt = morphAt + TIMING.morph;
    const doorsAt = lineAt + TIMING.line;
    const bounds = welcome.getBoundingClientRect();
    const pointScale = `scale(${1 / Math.max(bounds.width, 1)}, ${1 / Math.max(bounds.height, 1)})`;
    const lineHeight = overlay.getBoundingClientRect().height;

    welcome.style.visibility = "visible";
    animate(card, [
      { opacity: 1, transform: "scale(1)", filter: "blur(0)" },
      { opacity: 0, transform: "scale(.9)", filter: "blur(15px)" },
    ], TIMING.card, 0, "ease");
    const textMotion = animate(welcome, [
      { offset: 0, opacity: 0, transform: "scale(.95)", filter: "blur(10px)" },
      { offset: welcomeAt / lineAt, opacity: 0, transform: "scale(.95)", filter: "blur(10px)", easing: "ease-out" },
      { offset: (welcomeAt + TIMING.welcomeIn) / lineAt, opacity: 1, transform: "scale(1)", filter: "blur(0)" },
      { offset: morphAt / lineAt, opacity: 1, transform: "scale(1)", filter: "blur(0)", easing: EASE },
      { offset: 1, opacity: 1, transform: pointScale, filter: "blur(0)" },
    ], lineAt, 0, "linear");
    animate(point, [{ opacity: 0 }, { opacity: 1 }], 100, lineAt - 100, "linear");
    animate(point, [
      { transform: "translate(-50%, -50%) scaleY(1)" },
      { transform: `translate(-50%, -50%) scaleY(${lineHeight})` },
    ], TIMING.line, lineAt);
    animate(point, [{ opacity: 1 }, { opacity: 0 }], 60, doorsAt, "linear");
    for (const seam of seams) {
      animate(seam, [
        { offset: 0, opacity: 0 }, { offset: .02, opacity: 1 },
        { offset: .85, opacity: 1 }, { offset: 1, opacity: 0 },
      ], TIMING.doors, doorsAt, "linear");
    }
    doors.forEach((door, index) => {
      animate(door, [
        { transform: "translateX(0)" },
        { transform: `translateX(${index === 0 ? "-100%" : "100%"})` },
      ], TIMING.doors, doorsAt);
    });

    // Keep the text mounted until it reaches the same 1px center as the point.
    void textMotion.finished.then(() => {
      if (!cancelled) welcome.style.visibility = "hidden";
    }, () => {}); // Cancellation/failure is handled by the aggregate below.

    const finished = Promise.all(tracks.map(track => track.finished)).then(
      () => !cancelled,
      error => {
        if (cancelled) return false;
        cancel();
        throw error;
      },
    );
    return { finished, cancel };
  } catch (error) {
    cancel();
    throw error;
  }
}
