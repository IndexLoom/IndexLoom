import { useState } from "react";

import { AppShell, type View } from "./components/AppShell";
import { IndexersWorkspace } from "./components/IndexersWorkspace";
import { SearchWorkspace } from "./components/SearchWorkspace";

const viewCopy: Record<Exclude<View, "search" | "indexers">, { title: string; body: string }> = {
  history: {
    title: "History",
    body: "Completed searches will appear here when persistent history is enabled.",
  },
  settings: {
    title: "Settings",
    body: "Runtime preferences will appear here as the configuration API expands.",
  },
};

export function App() {
  const [view, setView] = useState<View>("search");

  return (
    <AppShell activeView={view} onNavigate={setView}>
      {view === "search" ? <SearchWorkspace /> : null}
      {view === "indexers" ? <IndexersWorkspace /> : null}
      {view === "history" || view === "settings" ? (
        <section className="empty-view">
          <h1>{viewCopy[view].title}</h1>
          <p>{viewCopy[view].body}</p>
        </section>
      ) : null}
    </AppShell>
  );
}
