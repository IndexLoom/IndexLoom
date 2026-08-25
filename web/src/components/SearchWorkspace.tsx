import { type FormEvent, useEffect, useState } from "react";

import { fetchHealth, type Health } from "../lib/api";

type HealthState =
  | { kind: "loading" }
  | { kind: "ready"; health: Health }
  | { kind: "error"; message: string };

export function SearchWorkspace() {
  const [health, setHealth] = useState<HealthState>({ kind: "loading" });
  const [query, setQuery] = useState("");
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    fetchHealth(controller.signal)
      .then((response) => setHealth({ kind: "ready", health: response }))
      .catch((error: unknown) => {
        if (!controller.signal.aborted) {
          setHealth({
            kind: "error",
            message: error instanceof Error ? error.message : "health check failed",
          });
        }
      });
    return () => controller.abort();
  }, []);

  function submitSearch(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setNotice(
      query.trim()
        ? "No enabled indexers are available yet. Add a definition before searching."
        : "Enter a search query first.",
    );
  }

  return (
    <section className="search-view" aria-labelledby="search-heading">
      <h1 className="sr-only" id="search-heading">
        Search
      </h1>

      <form className="search-toolbar" onSubmit={submitSearch}>
        <label className="sr-only" htmlFor="query">
          Search across indexers
        </label>
        <input
          id="query"
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Search across indexers"
          type="search"
          value={query}
        />
        <button className="select-button" type="button">
          All indexers
          <span aria-hidden="true">⌄</span>
        </button>
        <button className="primary-button" type="submit">
          Search
        </button>
      </form>

      {notice ? <p className="notice" role="status">{notice}</p> : null}

      <section className="data-section" aria-labelledby="status-heading">
        <div className="section-heading">
          <h2 id="status-heading">Indexer status</h2>
          <RuntimeStatus state={health} />
        </div>
        <div className="table-frame">
          <table>
            <thead>
              <tr>
                <th>Indexer</th>
                <th>Status</th>
                <th>Results</th>
                <th>Last checked</th>
                <th>Response time</th>
              </tr>
            </thead>
            <tbody>
              <tr className="empty-row">
                <td colSpan={5}>
                  No indexers configured. Definitions will be loaded from the separate Indexers repository.
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>

      <section className="data-section" aria-labelledby="results-heading">
        <div className="section-heading">
          <h2 id="results-heading">Results</h2>
          <span className="muted">0 items</span>
        </div>
        <div className="table-frame results-frame">
          <table>
            <thead>
              <tr>
                <th>Title</th>
                <th>Indexer</th>
                <th>Category</th>
                <th>Age</th>
                <th>Size</th>
                <th>Seeders</th>
              </tr>
            </thead>
            <tbody>
              <tr className="empty-row">
                <td colSpan={6}>Search results will appear here.</td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>
    </section>
  );
}

function RuntimeStatus({ state }: { state: HealthState }) {
  if (state.kind === "loading") {
    return <span className="runtime-status pending">Checking runtime</span>;
  }
  if (state.kind === "error") {
    return <span className="runtime-status error" title={state.message}>Runtime unavailable</span>;
  }
  return (
    <span className="runtime-status healthy">
      API {state.health.version} · database {state.health.database}
    </span>
  );
}
