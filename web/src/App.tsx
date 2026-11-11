import { useState } from "react";

import { AppShell, type View } from "./components/AppShell";
import { IndexersWorkspace } from "./components/IndexersWorkspace";
import { SearchWorkspace } from "./components/SearchWorkspace";
import { SettingsWorkspace } from "./components/SettingsWorkspace";

export function App() {
  const [view, setView] = useState<View>("search");

  return (
    <AppShell activeView={view} onNavigate={setView}>
      {view === "search" ? <SearchWorkspace /> : null}
      {view === "indexers" ? <IndexersWorkspace /> : null}
      {view === "settings" ? <SettingsWorkspace /> : null}
      {view === "history" ? (
        <section className="empty-view">
          <h1>History</h1>
          <p>Completed searches will appear here when persistent history is enabled.</p>
        </section>
      ) : null}
    </AppShell>
  );
}
