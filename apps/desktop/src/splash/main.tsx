import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import StrokeText from "./StrokeText";
import "./splash.css";

// The startup splash runs in its own transparent, undecorated window
// ("splashscreen" in tauri.conf.json). It only plays the StrokeText animation
// and reports when it has finished; the Rust side shows the main window once
// both this signal and the main window's first-paint signal have arrived.

let reported = false;
function reportAnimationDone() {
  if (reported) return;
  reported = true;
  invoke("splash_signal", { kind: "animation" }).catch(() => {
    // Outside Tauri (plain `vite` in a browser) there is nothing to notify.
  });
}

function Splash() {
  return (
    <div className="splash-root" data-tauri-drag-region>
      <StrokeText
        text="DeepAgent-Studio"
        strokeColor="#A78BFA"
        strokeStyle="solid"
        strokeWidth={1.4}
        dashLength={10}
        drawDuration={1.9}
        fillDelay={0.2}
        stagger={0.05}
        ease="power2.out"
        trigger="mount"
        fillMode="fade"
        wipeFrom="left"
        fontSize={128}
        fontWeight={800}
        letterSpacing={-4}
        marching={false}
        hoverRedraw={true}
        shadow={false}
        onComplete={reportAnimationDone}
      />
    </div>
  );
}

// Measure glyphs only after fonts settle: StrokeText rebuilds its timeline when
// the measured box changes, which would visibly restart the animation.
// No StrictMode here: its double effect run in dev would replay the animation.
const fontsReady: Promise<unknown> = document.fonts?.ready ?? Promise.resolve();
void fontsReady
  .catch(() => undefined)
  .then(() => {
    ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<Splash />);
  });
