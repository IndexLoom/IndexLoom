import {
  t, categoryLabel, canonicalGenre, canonicalGenres,
  genreLabel, searchModeLabel, formatDateTime, formatNumber,
} from "../lib/i18n";
import { type FormEvent, useEffect, useMemo, useRef, useState } from "react";

import {
  ApiError,
  type Health,
  type Indexer,
  type IndexerGroup,
  type IndexerSelection,
  type SearchIndexerResults,
  type SearchResponse,
  type SearchFilters,
  fetchGroups,
  fetchHealth,
  fetchIndexers,
  searchIndexersStream,
} from "../lib/api";
import { AdminTokenGate, useAdminSession } from "./AdminSession";

type WorkspaceState =
  | { kind: "loading" }
  | { kind: "error"; message: string }
  | {
      kind: "ready";
      health: Health;
      indexers: Indexer[];
      groups: IndexerGroup[];
    };

export function SearchWorkspace() {
  const { token, acceptToken, clearToken } = useAdminSession();
  if (!token) {
    return <AdminTokenGate onSubmit={acceptToken} />;
  }
  return <AuthenticatedSearch onUnauthorized={clearToken} token={token} />;
}

function AuthenticatedSearch({
  token,
  onUnauthorized,
}: {
  token: string;
  onUnauthorized: () => void;
}) {
  const [workspace, setWorkspace] = useState<WorkspaceState>({
    kind: "loading",
  });
  const [query, setQuery] = useState("");
  const [filters, setFilters] = useState<SearchFilters>({});
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [selection, setSelection] = useState<IndexerSelection>({
    all: true,
    groupIds: [],
    indexerIds: [],
  });
  const [searching, setSearching] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [response, setResponse] = useState<SearchResponse | null>(null);
  const [searchProgress, setSearchProgress] = useState<{
    completed: number;
    total: number;
  } | null>(null);
  const searchAbort = useRef<AbortController | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    Promise.all([
      fetchHealth(controller.signal),
      fetchIndexers(token, controller.signal),
      fetchGroups(token, controller.signal),
    ])
      .then(([health, indexers, groups]) =>
        setWorkspace({ kind: "ready", health, indexers, groups }),
      )
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        if (error instanceof ApiError && error.status === 401) {
          onUnauthorized();
          return;
        }
        setWorkspace({
          kind: "error",
          message:
            error instanceof Error ? error.message : "search workspace failed",
        });
      });
    return () => controller.abort();
  }, [onUnauthorized, token]);

  useEffect(
    () => () => {
      searchAbort.current?.abort();
    },
    [],
  );

  const enabledIndexers = useMemo(
    () =>
      workspace.kind === "ready"
        ? workspace.indexers.filter((indexer) => indexer.enabled && indexer.catalogAvailable)
        : [],
    [workspace],
  );
  const availableCategories = useMemo(
    () => [...new Set(enabledIndexers.flatMap((indexer) => indexer.capabilities.categories))].sort(),
    [enabledIndexers],
  );
  const availableModes = useMemo(
    () => [...new Set(enabledIndexers.flatMap((indexer) => indexer.capabilities.searchModes.map((mode) => mode.name)))].sort(),
    [enabledIndexers],
  );
  const groupNames = useMemo(
    () =>
      new Map(
        workspace.kind === "ready"
          ? workspace.groups.map((group) => [group.id, group.name])
          : [],
      ),
    [workspace],
  );

  async function submitSearch(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!query.trim()) {
      setNotice("Enter a search query first.");
      return;
    }
    searchAbort.current?.abort();
    const controller = new AbortController();
    const submittedQuery = query.trim();
    searchAbort.current = controller;
    setSearching(true);
    setNotice(null);
    setSearchProgress(null);
    const indexers = new Map<string, SearchIndexerResults>();
    let resultCount = 0;
    try {
      await searchIndexersStream(
        token,
        submittedQuery,
        selection,
        (event) => {
          if (event.type === "started") {
            setResponse({ query: event.query, indexers: [], resultCount: 0 });
            setSearchProgress({ completed: 0, total: event.total });
            return;
          }
          if (event.type === "indexer") {
            indexers.set(event.indexer.indexerInstanceId, event.indexer);
            const ordered = Array.from(indexers.values()).sort(compareIndexers);
            resultCount = ordered.reduce(
              (total, indexer) => total + indexer.results.length,
              0,
            );
            setResponse({
              query: submittedQuery,
              indexers: ordered,
              resultCount,
            });
            setSearchProgress((current) => ({
              completed: (current?.completed ?? 0) + 1,
              total: current?.total ?? ordered.length,
            }));
            return;
          }
          setSearchProgress({
            completed: event.completed,
            total: event.total,
          });
          setNotice(
            event.resultCount === 0
              ? "The selected indexers returned no results. Expand each indexer to inspect errors."
              : null,
          );
        },
        controller.signal,
        50,
        { ...filters, genre: filters.genre ? canonicalGenre(filters.genre) : undefined },
      );
    } catch (error) {
      if (controller.signal.aborted) return;
      if (error instanceof ApiError && error.status === 401) {
        onUnauthorized();
        return;
      }
      setNotice(error instanceof Error ? error.message : "search failed");
    } finally {
      if (searchAbort.current === controller) {
        searchAbort.current = null;
        setSearching(false);
      }
    }
  }

  if (workspace.kind === "loading") {
    return (
      <WorkspaceMessage
        title={t("Loading search")}
        body="Synchronizing indexers and groups."
      />
    );
  }
  if (workspace.kind === "error") {
    return (
      <WorkspaceMessage title={t("Search unavailable")} body={t(workspace.message)} />
    );
  }

  return (
    <section className="search-view" aria-labelledby="search-heading">
      <div className="page-heading">
        <div>
          <p className="eyebrow">{t("Federated torrent search")}</p>
          <h1 id="search-heading">{t("Search")}</h1>
          <p className="page-description">{t("Query every enabled source at once and inspect results as each indexer responds.")}{" "}</p>
        </div>
        <button className="quiet-button" onClick={onUnauthorized} type="button">{t("Lock")}{" "}</button>
      </div>

      <div className="overview-grid search-overview" aria-label={t("Search overview")} data-ui-section="overview">
        <div className="overview-metric healthy">
          <strong>{formatNumber(enabledIndexers.length)}</strong>
          <span>{t("Enabled indexers")}</span>
        </div>
        <div className="overview-metric">
          <strong>{workspace.groups.filter((group) => group.enabled).length}</strong>
          <span>{t("Active groups")}</span>
        </div>
        <div className="overview-metric">
          <strong>{response?.resultCount ?? 0}</strong>
          <span>{t("Current results")}</span>
        </div>
      </div>

      <form className="search-toolbar" data-ui-section="query" onSubmit={submitSearch}>
        <label className="sr-only" htmlFor="query">{t("Search across indexers")}{" "}</label>
        <input
          id="query"
          onChange={(event) => setQuery(event.target.value)}
          placeholder={t("Movie, series, game, book…")}
          type="search"
          value={query}
        />
        <SelectionMenu
          enabledIndexers={enabledIndexers}
          groups={workspace.groups}
          onChange={setSelection}
          selection={selection}
        />
        <button className="primary-button" disabled={searching} type="submit">
          {searching ? "Searching…" : "Search"}
        </button>
        <details className="advanced-search" open={advancedOpen} onToggle={(event) => setAdvancedOpen(event.currentTarget.open)}>
          <summary>{t("Advanced search filters")}</summary>
          <div className="advanced-search-grid">
            <label>{t("Mode")}{" "}<select value={filters.mode ?? ""} onChange={(event) => setFilters((current) => ({ ...current, mode: event.target.value || undefined }))}>
                <option value="">{t("Default search")}</option>
                {availableModes.map((mode) => <option key={mode} value={mode}>{searchModeLabel(mode)}</option>)}
              </select>
            </label>
            <label>{t("Category")}{" "}<select value={filters.category ?? ""} onChange={(event) => setFilters((current) => ({ ...current, category: event.target.value || undefined }))}>
                <option value="">{t("All categories")}</option>
                {availableCategories.map((category) => <option key={category} value={category}>{categoryLabel(category)}</option>)}
              </select>
            </label>
            <label>{t("Genre")}{" "}<input list="indexloom-genre-suggestions" value={filters.genre ?? ""} onChange={(event) => setFilters((current) => ({ ...current, genre: event.target.value || undefined }))} placeholder={t("Genre ID or name")} />
              <datalist id="indexloom-genre-suggestions">
                {canonicalGenres.map((genre) => <option key={genre} value={genreLabel(genre)} />)}
              </datalist>
            </label>
            <label>{t("Year")}{" "}<input min={1800} max={2100} type="number" value={filters.year ?? ""} onChange={(event) => setFilters((current) => ({ ...current, year: event.target.value ? Number(event.target.value) : undefined }))} />
            </label>
            <label>{t("Season")}{" "}<input min={0} max={1000} type="number" value={filters.season ?? ""} onChange={(event) => setFilters((current) => ({ ...current, season: event.target.value ? Number(event.target.value) : undefined }))} />
            </label>
            <label>{t("Episode")}{" "}<input min={0} max={10000} type="number" value={filters.episode ?? ""} onChange={(event) => setFilters((current) => ({ ...current, episode: event.target.value ? Number(event.target.value) : undefined }))} />
            </label>
            <label>{t("IMDb ID")}{" "}<input value={filters.imdbId ?? ""} placeholder="tt1234567" onChange={(event) => setFilters((current) => ({ ...current, imdbId: event.target.value || undefined }))} />
            </label>
            <label>{t("TMDb ID")}{" "}<input value={filters.tmdbId ?? ""} placeholder="12345" onChange={(event) => setFilters((current) => ({ ...current, tmdbId: event.target.value || undefined }))} />
            </label>
          </div>
          <p className="muted">{t("Search fields are sent only to indexers whose definitions support them. Unsupported filters may produce per-indexer errors.")}</p>
          <button className="quiet-button" onClick={() => setFilters({})} type="button">{t("Clear filters")}</button>
        </details>
      </form>

      {notice ? (
        <p className="notice" role="status">
          {t(notice)}
        </p>
      ) : null}

      <section className="data-section" data-ui-section="status" aria-labelledby="status-heading">
        <div className="section-heading">
          <h2 id="status-heading">{t("Indexer status")}</h2>
          <RuntimeStatus health={workspace.health} />
        </div>
        <div className="table-frame compact-status-table">
          <table>
            <thead>
              <tr>
                <th>{t("Indexer")}</th>
                <th>{t("State")}</th>
                <th>{t("Groups")}</th>
                <th>{t("Last test")}</th>
                <th>{t("Response")}</th>
              </tr>
            </thead>
            <tbody>
              {enabledIndexers.length === 0 ? (
                <tr className="empty-row">
                  <td colSpan={5}>{t("Enable at least one indexer in the Indexers workspace.")}{" "}</td>
                </tr>
              ) : (
                enabledIndexers.map((indexer) => (
                  <tr key={indexer.id}>
                    <td>
                      <strong>{indexer.displayName}</strong>
                      <small>{indexer.tags.join(" · ")}</small>
                    </td>
                    <td>
                      <span
                        className={`status-pill ${indexer.lastTest?.status === "success" ? "healthy" : ""}`}
                      >
                        {t(indexer.lastTest?.status ?? "not tested")}
                      </span>
                    </td>
                    <td>
                      {indexer.groupIds
                        .map((id) => groupNames.get(id) ?? id)
                        .join(", ") || "—"}
                    </td>
                    <td title={indexer.lastTest?.message}>
                      {formatDate(indexer.lastTest?.testedAt)}
                    </td>
                    <td>
                      {indexer.lastTest?.responseTimeMs == null
                        ? "—"
                        : `${indexer.lastTest.responseTimeMs} ms`}
                    </td>
                  </tr>
                ))
              )}
            </tbody>
          </table>
        </div>
      </section>

      <section className="data-section" data-ui-section="results" aria-labelledby="results-heading">
        <div className="section-heading">
          <h2 id="results-heading">{t("Results by indexer")}</h2>
          <span className="muted">
            {formatNumber(response?.resultCount ?? 0)} {t("items")}
            {searchProgress
              ? ` · ${searchProgress.completed}/${searchProgress.total} indexers`
              : ""}
          </span>
        </div>
        {response ? (
          <div className="result-groups">
            {response.indexers.map((indexer) => (
              <details
                className="result-indexer"
                key={indexer.indexerInstanceId}
                open
              >
                <summary>
                  <span>
                    {indexer.indexerName}
                    {indexer.definitionId ? (
                      <small className="result-definition">
                        {indexer.indexerInstanceId === indexer.definitionId
                          ? indexer.definitionId
                          : `${indexer.indexerInstanceId} · ${indexer.definitionId}`}
                      </small>
                    ) : null}
                  </span>
                  <span className="muted">
                    {indexer.error
                      ? t("error")
                      : `${formatNumber(indexer.results.length)} ${t("results")} · ${indexer.responseTimeMs} ms`}
                  </span>
                </summary>
                {indexer.error ? (
                  <p className="inline-error">{t(indexer.error)}</p>
                ) : null}
                {indexer.results.length > 0 ? (
                  <div className="table-frame results-frame">
                    <table>
                      <thead>
                        <tr>
                          <th>{t("Title")}</th>
                          <th>{t("Category")}</th>
                          <th>{t("Age")}</th>
                          <th>{t("Size")}</th>
                          <th>{t("Seeders")}</th>
                          <th>{t("Links")}</th>
                        </tr>
                      </thead>
                      <tbody>
                        {indexer.results.map((result) => (
                          <tr key={result.id}>
                            <td>
                              {result.detailsUrl ? (
                                <a
                                  href={result.detailsUrl}
                                  rel="noreferrer"
                                  target="_blank"
                                >
                                  {result.title}
                                </a>
                              ) : (
                                result.title
                              )}
                              {result.infoHash ? (
                                <small className="result-identity">
                                  {result.infoHash}
                                </small>
                              ) : null}
                            </td>
                            <td>{categoryLabel(result.category)}</td>
                            <td>{formatDate(result.publishedAt)}</td>
                            <td>{formatBytes(result.size)}</td>
                            <td>{result.seeders ?? "—"}</td>
                            <td>
                              <span className="result-actions">
                                {result.downloadUrl ? (
                                  <a
                                    href={result.downloadUrl}
                                    rel="noreferrer"
                                    target="_blank"
                                  >{t("Download")}{" "}</a>
                                ) : null}
                                {result.magnetUrl ? (
                                  <a href={result.magnetUrl}>{t("Magnet")}</a>
                                ) : null}
                                {!result.downloadUrl && !result.magnetUrl
                                  ? "—"
                                  : null}
                              </span>
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                ) : !indexer.error ? (
                  <p className="panel-empty">{t("No matching releases.")}</p>
                ) : null}
              </details>
            ))}
          </div>
        ) : (
          <div className="panel-empty">{t("Search results will be grouped here by indexer. Every group can be collapsed independently.")}{" "}</div>
        )}
      </section>
    </section>
  );
}

function compareIndexers(
  left: SearchIndexerResults,
  right: SearchIndexerResults,
): number {
  return (
    left.indexerName.localeCompare(right.indexerName, undefined, {
      sensitivity: "base",
    }) || left.indexerInstanceId.localeCompare(right.indexerInstanceId)
  );
}

function SelectionMenu({
  selection,
  groups,
  enabledIndexers,
  onChange,
}: {
  selection: IndexerSelection;
  groups: IndexerGroup[];
  enabledIndexers: Indexer[];
  onChange: (selection: IndexerSelection) => void;
}) {
  const label = selection.all
    ? `${t("All enabled")} (${formatNumber(enabledIndexers.length)})`
    : `${formatNumber(selection.groupIds.length)} ${t("Groups").toLocaleLowerCase()} · ${formatNumber(selection.indexerIds.length)} ${t("Indexers").toLocaleLowerCase()}`;

  function toggle(
    collection: "groupIds" | "indexerIds",
    id: string,
    checked: boolean,
  ) {
    const values = new Set(selection[collection]);
    if (checked) values.add(id);
    else values.delete(id);
    onChange({
      ...selection,
      all: false,
      [collection]: Array.from(values).sort(),
    });
  }

  return (
    <details className="selection-menu">
      <summary className="select-button">
        {label}
        <span aria-hidden="true">⌄</span>
      </summary>
      <div className="selection-popover">
        <label className="checkbox-field">
          <input
            checked={selection.all}
            onChange={(event) =>
              onChange({
                all: event.target.checked,
                groupIds: [],
                indexerIds: [],
              })
            }
            type="checkbox"
          />{t("All enabled indexers")}{" "}</label>
        <strong>{t("Groups")}</strong>
        {groups
          .filter((group) => group.enabled)
          .map((group) => (
            <label className="checkbox-field" key={group.id}>
              <input
                checked={selection.groupIds.includes(group.id)}
                disabled={selection.all}
                onChange={(event) =>
                  toggle("groupIds", group.id, event.target.checked)
                }
                type="checkbox"
              />
              {group.name} <small>{group.memberCount}</small>
            </label>
          ))}
        <strong>{t("Individual indexers")}</strong>
        <div className="selection-indexers">
          {enabledIndexers.map((indexer) => (
            <label className="checkbox-field" key={indexer.id}>
              <input
                checked={selection.indexerIds.includes(indexer.id)}
                disabled={selection.all}
                onChange={(event) =>
                  toggle("indexerIds", indexer.id, event.target.checked)
                }
                type="checkbox"
              />
              {indexer.displayName}
            </label>
          ))}
        </div>
      </div>
    </details>
  );
}

function RuntimeStatus({ health }: { health: Health }) {
  return (
    <span className="runtime-status healthy">
      API {health.version} · {t("database")} {t(health.database)}
    </span>
  );
}

function WorkspaceMessage({ title, body }: { title: string; body: string }) {
  return (
    <section className="empty-view">
      <h1>{title}</h1>
      <p>{body}</p>
    </section>
  );
}

function formatBytes(value: number | null): string {
  if (value == null) return "—";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let amount = value;
  let unit = 0;
  while (amount >= 1024 && unit < units.length - 1) {
    amount /= 1024;
    unit += 1;
  }
  return `${amount >= 10 || unit === 0 ? amount.toFixed(0) : amount.toFixed(1)} ${units[unit]}`;
}

function formatDate(value: string | undefined | null): string {
  if (!value) return "—";
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? value : formatDateTime(value);
}
