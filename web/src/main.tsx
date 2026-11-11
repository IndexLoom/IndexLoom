import { AppearanceProvider } from "./lib/ui-preferences";
import { AuthenticationProvider } from "./components/AdminSession";
import { registerPwa } from "./lib/pwa";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "./styles.css";
import "./appearance.css";

const root = document.getElementById("root");

if (!root) {
  throw new Error("root element was not found");
}

createRoot(root).render(
  <StrictMode>
    <AppearanceProvider>
      <AuthenticationProvider><App /></AuthenticationProvider>
    </AppearanceProvider>
  </StrictMode>,
);

registerPwa();
