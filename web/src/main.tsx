import { AppearanceProvider } from "./lib/ui-preferences";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "./styles.css";

const root = document.getElementById("root");

if (!root) {
  throw new Error("root element was not found");
}

createRoot(root).render(
  <StrictMode>
    <AppearanceProvider><App /></AppearanceProvider>
  </StrictMode>,
);
