import { translate, categoryLabel, languageLabel, searchModeLabel, formatNumber } from "../../lib/i18n";
const t = (message: string, variables?: Record<string, string | number>) => translate(message, variables, "indexers");
import {
  type FormEvent,
  useDeferredValue,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import {
  ApiError,
  type ConfigurationField,
  type ConfigurationValue,
  type FlareSolverrInstance,
  type FlareSolverrUpdate,
  type Indexer,
  type IndexerGroup,
  type IndexerGroupUpdate,
  type IndexerSelection,
  type IndexerTestJob,
  type OutboundProxy,
  type OutboundProxyUpdate,
  type ProxyKind,
  type RoutingSettings,
  fetchFlareSolverr,
  fetchGroups,
  fetchIndexerTestJob,
  fetchIndexers,
  fetchProxies,
  fetchRouting,
  removeGroup,
  removeFlareSolverr,
  removeProxy,
  saveGroup,
  saveFlareSolverr,
  saveIndexerConfiguration,
  saveIndexerOrganization,
  saveProxy,
  saveRouting,
  startIndexerTests,
} from "../../lib/api";
import { AdminTokenGate, useAdminSession } from "../AdminSession";

import { IndexerEditor } from "./IndexerEditor";
export function IndexerConfigurationPanel({
  indexers,
  connectors,
  proxies,
  groups,
  token,
  onSaved,
  onUnauthorized,
}: {
  indexers: Indexer[];
  connectors: FlareSolverrInstance[];
  proxies: OutboundProxy[];
  groups: IndexerGroup[];
  token: string;
  onSaved: (indexer: Indexer) => void;
  onUnauthorized: () => void;
}) {
  const [filter, setFilter] = useState("");
  const [statusFilter, setStatusFilter] = useState<
    "all" | "enabled" | "disabled" | "unavailable"
  >("all");
  const [limit, setLimit] = useState(60);
  const [sourceFilter, setSourceFilter] = useState("all");
  const [languageFilter, setLanguageFilter] = useState("all");
  const [categoryFilter, setCategoryFilter] = useState("all");
  const [modeFilter, setModeFilter] = useState("all");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const editorPanelRef = useRef<HTMLDivElement>(null);
  const deferredFilter = useDeferredValue(filter.trim().toLocaleLowerCase());
  const languages = useMemo(() => [...new Set(indexers.map((item) => item.capabilities.language))].sort(), [indexers]);
  const categories = useMemo(() => [...new Set(indexers.flatMap((item) => item.capabilities.categories))].sort(), [indexers]);
  const modes = useMemo(() => [...new Set(indexers.flatMap((item) => item.capabilities.searchModes.map((mode) => mode.name)))].sort(), [indexers]);
  const filtered = useMemo(
    () =>
      indexers.filter((indexer) => {
        const caps = indexer.capabilities;
        const haystack =
          `${indexer.displayName} ${indexer.definitionId} ${indexer.tags.join(" ")} ${caps.description} ${caps.language} ${caps.source} ${caps.categories.join(" ")} ${caps.searchModes.map((mode) => mode.name).join(" ")}`.toLocaleLowerCase();
        const matchesState =
          statusFilter === "all" ||
          (statusFilter === "enabled" && indexer.enabled && indexer.catalogAvailable) ||
          (statusFilter === "disabled" && !indexer.enabled && indexer.catalogAvailable) ||
          (statusFilter === "unavailable" && !indexer.catalogAvailable);
        return matchesState
          && haystack.includes(deferredFilter)
          && (sourceFilter === "all" || caps.source === sourceFilter)
          && (languageFilter === "all" || caps.language === languageFilter)
          && (categoryFilter === "all" || caps.categories.includes(categoryFilter))
          && (modeFilter === "all" || caps.searchModes.some((mode) => mode.name === modeFilter));
      }),
    [categoryFilter, deferredFilter, indexers, languageFilter, modeFilter, sourceFilter, statusFilter],
  );
  const visible = filtered.slice(0, limit);
  const selected = selectedId
    ? indexers.find((indexer) => indexer.id === selectedId) ?? null
    : null;

  function selectIndexer(id: string) {
    setSelectedId(id);
    if (window.matchMedia("(max-width: 51.25rem)").matches) {
      window.requestAnimationFrame(() => {
        editorPanelRef.current?.scrollIntoView({ block: "start" });
      });
    }
  }

  return (
    <section
      className="configuration-section catalog-section"
      aria-labelledby="tracker-config-heading"
    >
      <div className="section-heading stacked-heading">
        <div>
          <h2 id="tracker-config-heading">{t("Indexer catalog")}</h2>
          <p>{t("Select one definition to configure it. Credentials remain encrypted and are never returned by the API.")}{" "}</p>
        </div>
        <span className="muted">
          {t("Showing")} {formatNumber(filtered.length)} {t("of")} {formatNumber(indexers.length)}
        </span>
      </div>
      <div className="indexer-toolbar">
        <label className="catalog-filter">
          <span>{t("Find an indexer")}</span>
          <input
            onChange={(event) => {
              setFilter(event.target.value);
              setLimit(60);
            }}
            placeholder={t("Name, definition, or tag")}
            type="search"
            value={filter}
          />
        </label>
        <label className="catalog-filter status-filter">
          <span>{t("Status")}</span>
          <select
            onChange={(event) => {
              setStatusFilter(
                event.target.value as "all" | "enabled" | "disabled" | "unavailable",
              );
              setLimit(60);
            }}
            value={statusFilter}
          >
            <option value="all">{t("All indexers")}</option>
            <option value="enabled">{t("Enabled")}</option>
            <option value="disabled">{t("Disabled")}</option>
            <option value="unavailable">{t("Retired")}</option>
          </select>
        </label>
        <label className="catalog-filter">
          <span>{t("Source")}</span>
          <select value={sourceFilter} onChange={(event) => { setSourceFilter(event.target.value); setLimit(60); }}>
            <option value="all">{t("Both upstreams")}</option>
            <option value="jackett">Jackett</option>
            <option value="prowlarr-indexers">Prowlarr</option>
          </select>
        </label>
        <label className="catalog-filter">
          <span>{t("Language")}</span>
          <select value={languageFilter} onChange={(event) => { setLanguageFilter(event.target.value); setLimit(60); }}>
            <option value="all">{t("All languages")}</option>
            {languages.map((language) => <option key={language} value={language}>{languageLabel(language)}</option>)}
          </select>
        </label>
        <label className="catalog-filter">
          <span>{t("Category")}</span>
          <select value={categoryFilter} onChange={(event) => { setCategoryFilter(event.target.value); setLimit(60); }}>
            <option value="all">{t("All categories")}</option>
            {categories.map((category) => <option key={category} value={category}>{categoryLabel(category)}</option>)}
          </select>
        </label>
        <label className="catalog-filter">
          <span>{t("Search mode")}</span>
          <select value={modeFilter} onChange={(event) => { setModeFilter(event.target.value); setLimit(60); }}>
            <option value="all">{t("Any mode")}</option>
            {modes.map((mode) => <option key={mode} value={mode}>{searchModeLabel(mode)}</option>)}
          </select>
        </label>
      </div>
      {indexers.length === 0 ? (
        <div className="panel-empty">{t("No indexer instances are installed yet. Catalog synchronization installs definitions disabled by default.")}{" "}</div>
      ) : (
        <div className="indexer-browser">
          <div className="indexer-directory" aria-label={t("Available indexers")}>
            {visible.length === 0 ? (
              <p className="panel-empty">{t("No indexers match the current filters.")}{" "}</p>
            ) : null}
            {visible.map((indexer) => (
              <button
                aria-pressed={selectedId === indexer.id}
                className={
                  selectedId === indexer.id
                    ? "indexer-list-item active"
                    : "indexer-list-item"
                }
                key={indexer.id}
                onClick={() => selectIndexer(indexer.id)}
                type="button"
              >
                <span className="indexer-list-copy">
                  <strong>{indexer.displayName}</strong>
                  <small>{indexer.definitionId} · {languageLabel(indexer.capabilities.language)} · {indexer.capabilities.source === "jackett" ? "Jackett" : "Prowlarr"}</small>
                </span>
                <span className="indexer-list-meta">
                  <span className={`privacy-pill ${t(indexer.privacy)}`}>
                    {t(indexer.privacy)}
                  </span>
                  <span
                    className={
                      indexer.catalogAvailable && indexer.enabled
                        ? "status-pill healthy"
                        : "status-pill"
                    }
                  >
                    {t(!indexer.catalogAvailable
                      ? "unavailable"
                      : indexer.enabled
                        ? "enabled"
                        : "disabled")}
                  </span>
                </span>
              </button>
            ))}
            {visible.length < filtered.length ? (
              <div className="show-more">
                <button
                  className="quiet-button"
                  onClick={() => setLimit((current) => current + 60)}
                  type="button"
                >
                  {t("Show")} {formatNumber(Math.min(60, filtered.length - visible.length))} {t("more")}
                </button>
              </div>
            ) : null}
          </div>

          <div className="indexer-editor-panel" ref={editorPanelRef}>
            {selected ? (
              <IndexerEditor
                connectors={connectors}
                groups={groups}
                indexer={selected}
                key={selected.id}
                onClose={() => setSelectedId(null)}
                onSaved={onSaved}
                onUnauthorized={onUnauthorized}
                proxies={proxies}
                token={token}
              />
            ) : (
              <div className="editor-empty">
                <span className="editor-empty-mark" aria-hidden="true">
                  +
                </span>
                <h3>{t("Select an indexer")}</h3>
                <p>{t("Choose a definition from the catalog to enable it, assign routing, or add credentials.")}{" "}</p>
              </div>
            )}
          </div>
        </div>
      )}
    </section>
  );
}

