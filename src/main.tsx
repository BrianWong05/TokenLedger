import React, { useState } from "react";
import ReactDOM from "react-dom/client";
import Overview8b from "./overview/Overview8b";
import Limits from "./overview/Limits";
import "./index.css";

function App() {
  const [nav, setNav] = useState("Overview");
  return nav === "Limits" ? (
    <Limits nav={nav} onNav={setNav} />
  ) : (
    <Overview8b nav={nav} onNav={setNav} />
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
