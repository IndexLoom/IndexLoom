import { useState } from "react";
import { useLocale } from "./lib/i18n";

import { AppShell, type View } from "./components/AppShell";
import { NetworkStatus } from "./components/NetworkStatus";
import { IndexersWorkspace } from "./components/IndexersWorkspace";
import { SearchWorkspace } from "./components/SearchWorkspace";
import { SettingsWorkspace } from "./components/SettingsWorkspace";
import { HistoryWorkspace } from "./components/HistoryWorkspace";
import { useAdminSession } from "./components/AdminSession";

export function App() {
  useLocale(); // Re-render the active workspace when the language changes.
  const { historyScope } = useAdminSession();
  const [replayQuery, setReplayQuery] = useState("");
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
      {view === "search" ? <SearchWorkspace initialQuery={replayQuery} /> : null}
      {view === "indexers" ? <IndexersWorkspace /> : null}
      {view === "settings" ? <SettingsWorkspace /> : null}
      {view === "history" ? (historyScope
        ? <HistoryWorkspace key={historyScope} scope={historyScope} onReplay={(query) => { setReplayQuery(query); navigate("search"); }} />
        : <p className="muted" role="status">Verifying account before showing search history…</p>
      ) : null}
    </AppShell>
  );
}
