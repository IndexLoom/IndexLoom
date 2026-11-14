import { useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { translate, categoryLabel, languageLabel, searchModeLabel, formatNumber } from "../../lib/i18n";
import { ApiError, type FlareSolverrInstance, type Indexer, type IndexerGroup, type OutboundProxy,
  fetchIndexerTestJob, saveIndexerOrganization, startIndexerTests } from "../../lib/api";
const t = (message: string, variables?: Record<string, string | number>) => translate(message, variables, "indexers");

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
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [busy, setBusy] = useState<Record<string, boolean>>({});
  const [results, setResults] = useState<Record<string, string>>({});
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    if (!selectedId) return;
    const trigger = document.activeElement as HTMLElement | null;
    dialogRef.current?.showModal();
    return () => { dialogRef.current?.close(); trigger?.focus(); };
  }, [selectedId]);

  async function runAction(indexer: Indexer, action: "test" | "toggle") {
    if (busy[indexer.id]) return;
    setBusy(current => ({ ...current, [indexer.id]: true }));
    setResults(current => ({ ...current, [indexer.id]: t(action === "test" ? "Testing…" : "Saving changes…") }));
    try {
      if (action === "toggle") {
        const updated = await saveIndexerOrganization(token, indexer.id, {
          displayName: indexer.displayName, enabled: !indexer.enabled,
          proxyId: indexer.proxyId, groupIds: indexer.groupIds, tags: indexer.tags,
        });
        if (mounted.current) onSaved(updated);
        if (mounted.current) setResults(current => ({ ...current, [indexer.id]: t(updated.enabled ? "Enabled" : "Disabled") }));
      } else {
        let job = await startIndexerTests(token, { all: false, indexerIds: [indexer.id], groupIds: [] });
        const deadline = Date.now() + 180_000;
        while (job.status !== "completed" && mounted.current) {
          if (Date.now() > deadline) throw new Error(t("Test is still running. Check the Tests tab for completion."));
          await new Promise(resolve => window.setTimeout(resolve, 1000));
          if (!mounted.current) return;
          job = await fetchIndexerTestJob(token, job.id);
        }
        if (!mounted.current) return;
        const result = job.results.find(item => item.indexerId === indexer.id);
        setResults(current => ({ ...current, [indexer.id]: result
          ? `${t(result.success ? "success" : "failed")} · ${result.responseTimeMs} ms · ${result.message}`
          : t("No test result returned.") }));
      }
    } catch (error) {
      if (!mounted.current) return;
      if (error instanceof ApiError && error.status === 401) onUnauthorized();
      else setResults(current => ({ ...current, [indexer.id]: error instanceof Error ? error.message : t("Request failed") }));
    } finally {
      if (mounted.current) setBusy(current => ({ ...current, [indexer.id]: false }));
    }
  }
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
              <div className="il-indexer-row" key={indexer.id}>
                <div className="il-indexer-identity">
                  <strong>{indexer.displayName}</strong>
                  <small>{indexer.definitionId} · {languageLabel(indexer.capabilities.language)} · {indexer.capabilities.source === "jackett" ? "Jackett" : "Prowlarr"}</small>
                  <div className="il-tags">
                    <span>{t(indexer.privacy)}</span>
                    {indexer.capabilities.categories.map(category => <span key={category}>{categoryLabel(category)}</span>)}
                    {indexer.tags.map(tag => <span className="il-tag" key={tag}>#{tag}</span>)}
                  </div>
                </div>
                <div className="il-row-state">
                  <span className={indexer.catalogAvailable && indexer.enabled ? "status-pill healthy" : "status-pill"}>
                    {t(!indexer.catalogAvailable ? "Retired" : indexer.enabled ? "Enabled" : "Disabled")}
                  </span>
                  <small>{indexer.lastTest ? `${t(indexer.lastTest.status ?? (indexer.lastTest.success ? "success" : "failed"))} · ${indexer.lastTest.responseTimeMs ?? "—"} ms` : t("not tested")}</small>
                </div>
                <div className="il-row-actions" aria-label={indexer.displayName}>
                  {indexer.enabled && indexer.catalogAvailable ? (
                    <a className="il-action" href={`?view=search&indexer=${encodeURIComponent(indexer.id)}`}>{t("Search")}</a>
                  ) : (
                    <span className="il-action il-action-disabled" aria-disabled="true" title={t("Enable the indexer before searching")}>{t("Search")}</span>
                  )}
                  <button type="button" className="il-action" onClick={() => selectIndexer(indexer.id)}>{t("Configure")}</button>
                  <button type="button" className="il-action" disabled={busy[indexer.id] || !indexer.enabled || !indexer.catalogAvailable} onClick={() => void runAction(indexer, "test")}>{t("Test")}</button>
                  <button type="button" className="il-action" disabled={busy[indexer.id] || (!indexer.enabled && !indexer.catalogAvailable)} onClick={() => void runAction(indexer, "toggle")}>{t(indexer.enabled ? "Disable" : "Enable")}</button>
                </div>
                <p className="il-row-result" role="status">{results[indexer.id] ?? ""}</p>
              </div>
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

          {selected ? <dialog className="il-config-dialog" ref={dialogRef} aria-modal="true" aria-label={`${t("Indexer configuration")}: ${selected.displayName}`} onCancel={event => { event.preventDefault(); setSelectedId(null); }}>
              <IndexerEditor connectors={connectors} groups={groups} indexer={selected} key={selected.id}
                onClose={() => setSelectedId(null)} onSaved={onSaved} onUnauthorized={onUnauthorized} proxies={proxies} token={token} />
          </dialog> : null}
        </div>
      )}
    </section>
  );
}

