import { useState } from "react";

import { AppShell, type View } from "./components/AppShell";
import { SearchWorkspace } from "./components/SearchWorkspace";

const viewCopy: Record<Exclude<View, "search">, { title: string; body: string }> = {
  indexers: {
    title: "Indexers",
    body: "No definitions are installed. The first importer will consume the separate Indexers repository.",
  },
  history: {
    title: "History",
    body: "Search history is empty.",
  },
  settings: {
    title: "Settings",
    body: "Runtime settings will appear here as the configuration API is implemented.",
  },
};

export function App() {
  const [view, setView] = useState<View>("search");

  return (
    <AppShell activeView={view} onNavigate={setView}>
      {view === "search" ? (
        <SearchWorkspace />
      ) : (
        <section className="empty-view">
          <h1>{viewCopy[view].title}</h1>
          <p>{viewCopy[view].body}</p>
        </section>
      )}
    </AppShell>
  );
}
