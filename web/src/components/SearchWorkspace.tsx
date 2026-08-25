import { type FormEvent, useEffect, useMemo, useState } from "react";

import {
  ApiError,
  type Health,
  type Indexer,
  type IndexerGroup,
  type IndexerSelection,
  type SearchResponse,
  fetchGroups,
  fetchHealth,
  fetchIndexers,
  searchIndexers,
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
  const [selection, setSelection] = useState<IndexerSelection>({
    all: true,
    groupIds: [],
    indexerIds: [],
  });
  const [searching, setSearching] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [response, setResponse] = useState<SearchResponse | null>(null);

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

  const enabledIndexers = useMemo(
    () =>
      workspace.kind === "ready"
        ? workspace.indexers.filter((indexer) => indexer.enabled)
        : [],
    [workspace],
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
    setSearching(true);
    setNotice(null);
    try {
      const result = await searchIndexers(token, query.trim(), selection);
      setResponse(result);
      setNotice(
        result.resultCount === 0
          ? "The selected indexers returned no results. Expand each indexer to inspect errors."
          : null,
      );
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) {
        onUnauthorized();
        return;
      }
      setNotice(error instanceof Error ? error.message : "search failed");
    } finally {
      setSearching(false);
    }
  }

  if (workspace.kind === "loading") {
    return (
      <WorkspaceMessage
        title="Loading search"
        body="Synchronizing indexers and groups."
      />
    );
  }
  if (workspace.kind === "error") {
    return (
      <WorkspaceMessage title="Search unavailable" body={workspace.message} />
    );
  }

  return (
    <section className="search-view" aria-labelledby="search-heading">
      <div className="page-heading">
        <div>
          <p className="eyebrow">Federated torrent search</p>
          <h1 id="search-heading">Search</h1>
        </div>
        <button className="quiet-button" onClick={onUnauthorized} type="button">
          Lock
        </button>
      </div>

      <form className="search-toolbar" onSubmit={submitSearch}>
        <label className="sr-only" htmlFor="query">
          Search across indexers
        </label>
        <input
          id="query"
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Movie, series, game, book…"
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
      </form>

      {notice ? (
        <p className="notice" role="status">
          {notice}
        </p>
      ) : null}

      <section className="data-section" aria-labelledby="status-heading">
        <div className="section-heading">
          <h2 id="status-heading">Indexer status</h2>
          <RuntimeStatus health={workspace.health} />
        </div>
        <div className="table-frame compact-status-table">
          <table>
            <thead>
              <tr>
                <th>Indexer</th>
                <th>State</th>
                <th>Groups</th>
                <th>Last test</th>
                <th>Response</th>
              </tr>
            </thead>
            <tbody>
              {enabledIndexers.length === 0 ? (
                <tr className="empty-row">
                  <td colSpan={5}>
                    Enable at least one indexer in the Indexers workspace.
                  </td>
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
                        {indexer.lastTest?.status ?? "not tested"}
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

      <section className="data-section" aria-labelledby="results-heading">
        <div className="section-heading">
          <h2 id="results-heading">Results by indexer</h2>
          <span className="muted">{response?.resultCount ?? 0} items</span>
        </div>
        {response ? (
          <div className="result-groups">
            {response.indexers.map((indexer) => (
              <details className="result-indexer" key={indexer.indexerId} open>
                <summary>
                  <span>{indexer.indexerName}</span>
                  <span className="muted">
                    {indexer.error
                      ? "error"
                      : `${indexer.results.length} results · ${indexer.responseTimeMs} ms`}
                  </span>
                </summary>
                {indexer.error ? (
                  <p className="inline-error">{indexer.error}</p>
                ) : null}
                {indexer.results.length > 0 ? (
                  <div className="table-frame results-frame">
                    <table>
                      <thead>
                        <tr>
                          <th>Title</th>
                          <th>Category</th>
                          <th>Age</th>
                          <th>Size</th>
                          <th>Seeders</th>
                        </tr>
                      </thead>
                      <tbody>
                        {indexer.results.map((result) => (
                          <tr key={result.id}>
                            <td>
                              <a
                                href={result.link}
                                rel="noreferrer"
                                target="_blank"
                              >
                                {result.title}
                              </a>
                            </td>
                            <td>{result.category ?? "—"}</td>
                            <td>{formatDate(result.publishedAt)}</td>
                            <td>{formatBytes(result.size)}</td>
                            <td>{result.seeders ?? "—"}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                ) : !indexer.error ? (
                  <p className="panel-empty">No matching releases.</p>
                ) : null}
              </details>
            ))}
          </div>
        ) : (
          <div className="panel-empty">
            Search results will be grouped here by indexer. Every group can be
            collapsed independently.
          </div>
        )}
      </section>
    </section>
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
    ? `All enabled (${enabledIndexers.length})`
    : `${selection.groupIds.length} groups · ${selection.indexerIds.length} indexers`;

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
          />
          All enabled indexers
        </label>
        <strong>Groups</strong>
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
        <strong>Individual indexers</strong>
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
      API {health.version} · database {health.database}
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
  return Number.isNaN(date.valueOf()) ? value : date.toLocaleString();
}
