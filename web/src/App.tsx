import { useState } from "react";
import { t, useLocale } from "./lib/i18n";

import { AppShell, type View } from "./components/AppShell";
import { NetworkStatus } from "./components/NetworkStatus";
import { IndexersWorkspace } from "./components/IndexersWorkspace";
import { SearchWorkspace } from "./components/SearchWorkspace";
import { SettingsWorkspace } from "./components/SettingsWorkspace";

export function App() {
  useLocale(); // Re-render the active workspace when the language changes.
  const [view, setView] = useState<View>(() => {
    const value = new URLSearchParams(window.location.search).get("view");
    return value === "indexers" || value === "settings" || value === "history" ? value : "search";
  });
  function navigate(next: View) {
    setView(next);
    const url = new URL(window.location.href);
    url.searchParams.set("view", next);
    window.history.replaceState(null, "", url.pathname + url.search + url.hash);
  }

  return (
    <AppShell activeView={view} onNavigate={navigate}>
      <NetworkStatus />
      {view === "search" ? <SearchWorkspace /> : null}
      {view === "indexers" ? <IndexersWorkspace /> : null}
      {view === "settings" ? <SettingsWorkspace /> : null}
      {view === "history" ? (
        <section className="empty-view">
          <h1>{t("History")}</h1>
          <p>{t("Completed searches will appear here when persistent history is enabled.")}</p>
        </section>
      ) : null}
    </AppShell>
  );
}
