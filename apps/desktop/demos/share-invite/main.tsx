import React from "react";
import { createRoot } from "react-dom/client";
import { ShareInvite } from "./ShareInvite";
import "./share-invite.css";

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ShareInvite />
  </React.StrictMode>,
);
