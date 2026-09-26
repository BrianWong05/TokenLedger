import React from "react";
import ReactDOM from "react-dom/client";
import { loadRootModule } from "./mainRoot";
import "./index.css";

// The traypanel window (tauri.conf.json) loads index.html?panel=1 and gets
// the Menu Bar Extra panel instead of the app shell.
const isPanel = new URLSearchParams(window.location.search).has("panel");
// PROTOTYPE (panel-metric switch): dev-only harness, `?prototype=panel-metric`.
const prototype = import.meta.env.DEV
  ? new URLSearchParams(window.location.search).get("prototype")
  : null;
const root =
  prototype === "panel-metric"
    ? import("./traypanel/panelMetric.prototype")
    : loadRootModule(isPanel);

void root.then(({ default: Root }) => {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <Root />
    </React.StrictMode>,
  );
});
