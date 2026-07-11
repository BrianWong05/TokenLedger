import React from "react";
import ReactDOM from "react-dom/client";
import Limits from "./overview/Limits";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Limits nav="Limits" onNav={() => {}} />
  </React.StrictMode>,
);
