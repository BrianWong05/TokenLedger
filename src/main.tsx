import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import TrayPanel from "./traypanel/TrayPanel";
import "./index.css";

// The traypanel window (tauri.conf.json) loads index.html?panel=1 and gets
// the Menu Bar Extra panel instead of the app shell.
const isPanel = new URLSearchParams(window.location.search).has("panel");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isPanel ? <TrayPanel /> : <App />}
  </React.StrictMode>,
);
