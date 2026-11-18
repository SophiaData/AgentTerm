import { createApp } from "./app";
import { backend } from "./api";
import "./styles.css";

const root = document.getElementById("root");
if (!root) {
  throw new Error("AgentTerm root element is missing");
}

createApp(root, backend);
