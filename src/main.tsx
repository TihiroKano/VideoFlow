import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";

import "./design/tokens.css";
import "./design/base.css";
import "./design/glass.css";
import "./design/layout.css";
import "./design/components.css";

const container = document.getElementById("root");
if (!container) {
  throw new Error("找不到 #root 挂载点");
}

ReactDOM.createRoot(container).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);