import { type ChangeEvent, type DragEvent, useEffect, useRef, useState } from "react";

import { AdminTokenGate, useAdminSession } from "./AdminSession";
import { SecuritySettings } from "./SecuritySettings";
import { EmailSettings } from "./EmailSettings";
import { ApiError, type CatalogStatus, fetchCatalogStatus } from "../lib/api";
import {
  type PaletteKey, type UiPreferences, type Workspace,
  defaults, indexerBlocks, move, navigationPages, paletteFor,
  paletteKeys, searchBlocks, themes, useAppearance,
} from "../lib/ui-preferences";

type Tab = "themes" | "layout" | "css" | "database" | "security" | "email";
const labels: Record<Tab, string> = {
  themes: "Appearance", layout: "Layout", css: "Custom CSS", database: "Indexer database",
  security: "Security & users", email: "Email & notifications",
};
const paletteLabels: Record<PaletteKey, string> = {
  page: "Background", surface: "Surface", raised: "Elevated surface",
  sunken: "Input background", text: "Primary text", muted: "Secondary text",
  border: "Borders", accent: "Accent",
};

export function SettingsWorkspace() {
  const { preferences, update, importProfile, reset } = useAppearance();
  const [tab, setTab] = useState<Tab>("themes");
  const [message, setMessage] = useState("");
  const importInput = useRef<HTMLInputElement>(null);
  const palette = paletteFor(preferences);
  const set = <K extends keyof UiPreferences>(field: K, value: UiPreferences[K]) =>
    update((prev) => ({ ...prev, [field]: value }));

  async function importFile(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    if (!file) return;
    try {
      if (file.size > 120_000) throw new Error("Profile file is too large.");
      importProfile(JSON.parse(await file.text()) as unknown);
      setMessage("Appearance profile imported.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Invalid profile.");
    } finally {
      event.target.value = "";
    }
  }

  function exportProfile() {
    const blob = new Blob([JSON.stringify(preferences, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = "indexloom-appearance.json";
    anchor.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 2000);
  }

  return (
    <section className="settings-view appearance-settings">
      <header className="page-heading">
        <div>
          <p className="eyebrow">Personal workspace</p>
          <h1>Settings</h1>
          <p className="page-description">Make IndexLoom look and behave the way you want.</p>
        </div>
      </header>
      <div className="appearance-tabs" role="tablist" aria-label="Settings sections">
        {(Object.keys(labels) as Tab[]).map((key) => (
          <button
            aria-selected={key === tab}
            className={tab === key ? "active" : ""}
            key={key}
            onClick={() => setTab(key)}
            role="tab"
            type="button"
          >{labels[key]}</button>
        ))}
      </div>

      {tab === "themes" ? (
        <div className="appearance-content" role="tabpanel">
          <div className="setting-heading">
            <h2>Theme</h2>
            <p>Choose a starting point. Every color can be overridden independently.</p>
          </div>
          <div className="theme-grid">
            {(Object.keys(themes) as Array<keyof typeof themes>).map((theme) => {
              const colors = themes[theme].colors;
              return (
                <button
                  aria-pressed={preferences.theme === theme}
                  className={preferences.theme === theme ? "theme-swatch active" : "theme-swatch"}
                  key={theme}
                  onClick={() => update((prev) => ({ ...prev, theme, colors: {} }))}
                  type="button"
                >
                  <span className="theme-preview" style={{ background: colors[0], borderColor: colors[6] }}>
                    <span style={{ background: colors[1] }}>
                      <i style={{ background: colors[7] }} />
                      <i style={{ background: colors[2] }} />
                    </span>
                  </span>
                  <span>{themes[theme].name}</span>
                </button>
              );
            })}
          </div>
          <div className="setting-heading">
            <h2>Custom palette</h2>
            <p>Change individual tones without editing a stylesheet.</p>
          </div>
          <div className="palette-grid">
            {paletteKeys.map((key) => (
              <label className="palette-editor" key={key}>
                <span>{paletteLabels[key]}</span>
                <span className="palette-input">
                  <input
                    aria-label={paletteLabels[key]}
                    onChange={(event) => set("colors", { ...preferences.colors, [key]: event.target.value })}
                    type="color"
                    value={palette[key]}
                  />
                  <code>{palette[key]}</code>
                </span>
              </label>
            ))}
          </div>
          <div className="setting-heading"><h2>Shape and spacing</h2></div>
          <div className="appearance-controls">
            <label>
              <span>Density</span>
              <select onChange={(event) => set("density", event.target.value as UiPreferences["density"])} value={preferences.density}>
                <option value="comfortable">Comfortable</option>
                <option value="compact">Compact</option>
              </select>
            </label>
            <label>
              <span>Corner radius: {preferences.radius}px</span>
              <input max="24" min="0" onChange={(event) => set("radius", Number(event.target.value))} type="range" value={preferences.radius} />
            </label>
            <label>
              <span>Text size: {Math.round(preferences.fontScale * 100)}%</span>
              <input max="1.3" min="0.85" onChange={(event) => set("fontScale", Number(event.target.value))} step="0.05" type="range" value={preferences.fontScale} />
            </label>
          </div>
        </div>
      ) : null}

      {tab === "layout" ? (
        <div className="appearance-content" role="tabpanel">
          <div className="setting-heading">
            <h2>Navigation</h2>
            <p>Move the sidebar and reorder pages using drag and drop or arrow buttons.</p>
          </div>
          <div className="appearance-controls">
            <label><span>Sidebar position</span>
              <select onChange={(event) => set("sidebar", event.target.value as UiPreferences["sidebar"])} value={preferences.sidebar}>
                <option value="left">Left</option>
                <option value="right">Right</option>
                <option value="top">Top</option>
              </select>
            </label>
            <label>
              <span>Sidebar width: {preferences.sidebarWidth}px</span>
              <input max="350" min="170" onChange={(event) => set("sidebarWidth", Number(event.target.value))} type="range" value={preferences.sidebarWidth} />
            </label>
            <label>
              <span>Indexer list width: {preferences.indexerSplit}%</span>
              <input max="75" min="25" onChange={(event) => set("indexerSplit", Number(event.target.value))} type="range" value={preferences.indexerSplit} />
            </label>
          </div>
          <OrderEditor
            items={navigationPages.map((name) => ({ id: name, label: name.charAt(0).toUpperCase() + name.slice(1) }))}
            onChange={(navigation) => set("navigation", navigation)}
            order={preferences.navigation}
          />
          <div className="setting-heading">
            <h2>Search workspace</h2>
            <p>Arrange or hide the statistics, search controls, source status and results.</p>
          </div>
          <WorkspaceEditor kind="search" labels={searchBlocks.map((id) => ({ id, label: { overview: "Statistics", query: "Search controls", status: "Indexer status", results: "Search results" }[id] }))} />
          <div className="setting-heading">
            <h2>Indexer workspace</h2>
            <p>Rearrange the overview, management tabs and active panel. Your indexer data stays unchanged.</p>
          </div>
          <WorkspaceEditor kind="indexers" labels={indexerBlocks.map((id) => ({ id, label: { overview: "Statistics", tools: "Management tabs", panel: "Active panel" }[id] }))} />
        </div>
      ) : null}

      {tab === "css" ? (
        <div className="appearance-content" role="tabpanel">
          <div className="setting-heading">
            <h2>Your own CSS</h2>
            <p>Overrides are applied to this browser only. Import or export a profile to move the design to another device.</p>
          </div>
          <label className="custom-css-editor">
            <span>Custom stylesheet</span>
            <textarea
              onChange={(event) => set("customCss", event.target.value)}
              placeholder={".sidebar { border-right: 0; }\n.result-indexer { border-radius: 2px; }"}
              spellCheck={false}
              value={preferences.customCss}
            />
          </label>
          <p className="muted">Use CSS variables such as <code>--accent</code>, <code>--surface</code>, and <code>--ui-radius</code>. Custom CSS can make controls inaccessible or load remote resources; only import trusted profiles.</p>
        </div>
      ) : null}

      {tab === "database" ? <CatalogDiagnostics /> : null}
      {tab === "security" ? <SecuritySettings /> : null}
      {tab === "email" ? <EmailSettings /> : null}
      <footer className="appearance-footer">
        <div>
          <button className="quiet-button" onClick={exportProfile} type="button">Export profile</button>
          <button className="quiet-button" onClick={() => importInput.current?.click()} type="button">Import profile</button>
          <input accept=".json,application/json" aria-label="Import appearance profile" onChange={(event) => { void importFile(event); }} ref={importInput} style={{ display: "none" }} type="file" />
        </div>
        <button className="quiet-button" onClick={() => { reset(); setMessage("Appearance reset to Obsidian Violet."); }} type="button">Reset appearance</button>
      </footer>
      {message ? <p className="notice" role="status">{message}</p> : null}
    </section>
  );
}

function WorkspaceEditor({ kind, labels }: { kind: Workspace; labels: Array<{ id: string; label: string }> }) {
  const { preferences, update } = useAppearance();
  const order = preferences.blocks[kind];
  const hidden = preferences.hiddenBlocks[kind];
  return (
    <OrderEditor
      hidden={hidden}
      items={labels}
      onChange={(next) => update((current) => ({ ...current, blocks: { ...current.blocks, [kind]: next } }))}
      onToggle={(id) => update((current) => ({
        ...current,
        hiddenBlocks: {
          ...current.hiddenBlocks,
          [kind]: hidden.includes(id) ? hidden.filter((value) => value !== id) : [...hidden, id],
        },
      }))}
      order={order}
    />
  );
}

function OrderEditor({ items, order, hidden, onChange, onToggle }: {
  items: Array<{ id: string; label: string }>;
  order: string[];
  hidden?: string[];
  onChange: (order: string[]) => void;
  onToggle?: (id: string) => void;
}) {
  const dragging = useRef<string | null>(null);
  return (
    <div className="block-order-list">
      {order.map((id, index) => {
        const item = items.find((value) => value.id === id);
        if (!item) return null;
        const dropped = (event: DragEvent<HTMLDivElement>) => {
          event.preventDefault();
          if (dragging.current) onChange(move(order, dragging.current, id));
          dragging.current = null;
        };
        return (
          <div className="block-order-item" draggable key={id}
            onDragStart={() => { dragging.current = id; }}
            onDragOver={(event) => event.preventDefault()}
            onDrop={dropped}
          >
            <span className="block-grip" aria-hidden="true">⠿</span>
            <strong>{item.label}</strong>
            {hidden && onToggle ? (
              <label className="visibility-check">
                <input checked={!hidden.includes(id)} onChange={() => onToggle(id)} type="checkbox" />
                Visible
              </label>
            ) : null}
            <span className="order-actions">
              <button aria-label={"Move " + item.label + " up"} disabled={index === 0} onClick={() => onChange(move(order, id, order[index - 1] ?? id))} type="button">↑</button>
              <button aria-label={"Move " + item.label + " down"} disabled={index === order.length - 1} onClick={() => onChange(move(order, id, order[index + 1] ?? id))} type="button">↓</button>
            </span>
          </div>
        );
      })}
    </div>
  );
}

function CatalogDiagnostics() {
  const { token, acceptToken, clearToken } = useAdminSession();
  const [status, setStatus] = useState<CatalogStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [refreshKey, setRefreshKey] = useState(0);
  useEffect(() => {
    if (!token) return;
    const controller = new AbortController();
    setLoading(true);
    setError(null);
    fetchCatalogStatus(token, controller.signal)
      .then((value) => { setStatus(value); setError(null); })
      .catch((reason: unknown) => {
        if (!controller.signal.aborted) setError(reason instanceof ApiError ? reason.message : "Could not load catalog status.");
      })
      .finally(() => { if (!controller.signal.aborted) setLoading(false); });
    return () => controller.abort();
  }, [token, refreshKey]);
  if (!token) return <AdminTokenGate onSubmit={acceptToken} />;
  return (
    <section className="appearance-content" role="tabpanel">
      <div className="setting-heading">
        <h2>Indexer database</h2>
        <p>Installed catalog statistics and the last successful background synchronization.</p>
      </div>
      {status ? (
        <dl className="catalog-stats">
          <div><dt>Available</dt><dd>{status.availableDefinitions.toLocaleString()}</dd></div>
          <div><dt>Retired</dt><dd>{status.retiredDefinitions.toLocaleString()}</dd></div>
          <div><dt>Stored</dt><dd>{status.totalDefinitions.toLocaleString()}</dd></div>
          <div><dt>Last applied release</dt><dd>{status.latestReleaseAppliedAt ? new Date(status.latestReleaseAppliedAt).toLocaleString() : "Not yet recorded"}</dd></div>
        </dl>
      ) : null}
      {error ? <p className="inline-error" role="alert">{error}</p> : null}
      <button className="quiet-button" disabled={loading} onClick={() => setRefreshKey((key) => key + 1)} type="button">
        {loading ? "Checking…" : "Refresh status"}
      </button>
      <button className="quiet-button" onClick={clearToken} type="button">Lock database diagnostics</button>
      <p className="muted">Refresh reloads diagnostics only; the server handles database updates automatically.</p>
    </section>
  );
}
