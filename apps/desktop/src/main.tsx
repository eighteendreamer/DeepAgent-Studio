import React, { lazy, Suspense } from "react";
import ReactDOM from "react-dom/client";
import "./i18n";
import "./styles.css";
import { bootstrapTheme } from "./theme/themeBootstrap";
import { ThemeProvider } from "./theme/ThemeProvider";

bootstrapTheme();

const MainApp = lazy(() => import("./App").then((module) => ({ default: module.App })));
const StudioCanvasApp = lazy(() =>
  import("./canvas/CanvasApp").then((module) => ({ default: module.CanvasApp })),
);

function resolveRootView() {
  const rawHash = window.location.hash.startsWith("#")
    ? window.location.hash.slice(1)
    : window.location.hash;
  const params = new URLSearchParams(rawHash);
  return params.get("window") === "canvas" ? <StudioCanvasApp /> : <MainApp />;
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider>
      <Suspense fallback={<div className="h-screen w-full bg-sidebar-bg" />}>
        {resolveRootView()}
      </Suspense>
    </ThemeProvider>
  </React.StrictMode>
);
