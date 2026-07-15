import React from "react";
import ReactDOM from "react-dom/client";
import Overview from "./overview/Overview";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Overview />
  </React.StrictMode>,
);
