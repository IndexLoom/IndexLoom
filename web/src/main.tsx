import { AppearanceProvider } from "./lib/ui-preferences";
import { LocaleProvider } from "./lib/i18n";
import { AuthenticationProvider } from "./components/AdminSession";
import { registerPwa } from "./lib/pwa";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "./styles.css";
import "./appearance.css";
import "./auth-responsive.css";

const root = document.getElementById("root");

if (!root) {
  throw new Error("root element was not found");
}

createRoot(root).render(
  <StrictMode>
    <LocaleProvider>
      <AppearanceProvider>
        <AuthenticationProvider><App /></AuthenticationProvider>
      </AppearanceProvider>
    </LocaleProvider>
  </StrictMode>,
);

registerPwa();
