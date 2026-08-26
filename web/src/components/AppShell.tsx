import type { ReactNode } from "react";

export type View = "search" | "indexers" | "history" | "settings";

type AppShellProps = {
  activeView: View;
  children: ReactNode;
  onNavigate: (view: View) => void;
};

const navigation: Array<{ id: View; label: string }> = [
  { id: "search", label: "Search" },
  { id: "indexers", label: "Indexers" },
  { id: "history", label: "History" },
  { id: "settings", label: "Settings" },
];

export function AppShell({ activeView, children, onNavigate }: AppShellProps) {
  return (
    <div className="app-shell">
      <a className="skip-link" href="#main-workspace">
        Skip to workspace
      </a>
      <aside className="sidebar">
        <div className="brand" aria-label="IndexLoom">
          <span className="brand-mark" aria-hidden="true">
            <i />
            <i />
            <i />
            <i />
          </span>
          <span>IndexLoom</span>
        </div>

        <nav className="navigation" aria-label="Primary navigation">
          {navigation.map((item) => (
            <button
              aria-current={item.id === activeView ? "page" : undefined}
              className={item.id === activeView ? "nav-item active" : "nav-item"}
              key={item.id}
              onClick={() => onNavigate(item.id)}
              title={item.label}
              type="button"
            >
              <NavIcon view={item.id} />
              <span>{item.label}</span>
            </button>
          ))}
        </nav>

        <p className="sidebar-footnote">Pre-alpha · schema v1alpha1</p>
      </aside>
      <main className="workspace" id="main-workspace" tabIndex={-1}>
        {children}
      </main>
    </div>
  );
}

function NavIcon({ view }: { view: View }) {
  const common = {
    "aria-hidden": true,
    fill: "none",
    stroke: "currentColor",
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    strokeWidth: 1.7,
    viewBox: "0 0 24 24",
  };

  if (view === "search") {
    return (
      <svg {...common}>
        <circle cx="10.8" cy="10.8" r="6.4" />
        <path d="m15.6 15.6 4 4" />
      </svg>
    );
  }
  if (view === "indexers") {
    return (
      <svg {...common}>
        <ellipse cx="12" cy="5.3" rx="7" ry="3" />
        <path d="M5 5.3v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6M5 11.3v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6" />
      </svg>
    );
  }
  if (view === "history") {
    return (
      <svg {...common}>
        <circle cx="12" cy="12" r="8" />
        <path d="M12 7.6v4.8l3.2 2" />
      </svg>
    );
  }
  return (
    <svg {...common}>
      <circle cx="12" cy="12" r="3" />
      <path d="M19 12a7 7 0 0 0-.1-1l2-1.6-2-3.4-2.5 1a8 8 0 0 0-1.7-1L14.3 3h-4.6L9.3 6a8 8 0 0 0-1.7 1L5.1 6 3 9.4 5.1 11a7 7 0 0 0 0 2L3 14.6 5.1 18l2.5-1a8 8 0 0 0 1.7 1l.4 3h4.6l.4-3a8 8 0 0 0 1.7-1l2.5 1 2.1-3.4-2.1-1.6a7 7 0 0 0 .1-1Z" />
    </svg>
  );
}
