import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import ErrorBoundary from "./ErrorBoundary";
import "./styles.css";
import "@xterm/xterm/css/xterm.css";

const root = document.getElementById("root");

if (!root) {
  throw new Error("AgentTerm root element is missing");
}

createRoot(root).render(
  <StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </StrictMode>,
);
