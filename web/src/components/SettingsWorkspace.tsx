import { useEffect, useState } from "react";

import { AdminTokenGate, useAdminSession } from "./AdminSession";
import { ApiError, type CatalogStatus, fetchCatalogStatus } from "../lib/api";

export function SettingsWorkspace() {
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
      .then((value) => {
        setStatus(value);
        setError(null);
      })
      .catch((reason: unknown) => {
        if (controller.signal.aborted) return;
        setError(
          reason instanceof ApiError
            ? reason.message
            : "Could not load catalog status.",
        );
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
    return () => controller.abort();
  }, [token, refreshKey]);

  if (!token) return <AdminTokenGate onSubmit={acceptToken} />;

  return (
    <section className="settings-view">
      <header className="page-heading">
        <div>
          <p className="eyebrow">Application settings</p>
          <h1>Settings</h1>
        </div>
        <button className="quiet-button" onClick={clearToken} type="button">
          Lock workspace
        </button>
      </header>
      <div className="configuration-section">
        <div className="stacked-heading">
          <h2>Indexer database</h2>
          <p>
            IndexLoom verifies independent Indexers releases. These counts
            reflect your installed database, not all trackers in the world.
          </p>
        </div>
        {status ? (
          <dl className="catalog-stats">
            <div>
              <dt>Available definitions</dt>
              <dd>{status.availableDefinitions.toLocaleString()}</dd>
            </div>
            <div>
              <dt>Retired definitions</dt>
              <dd>{status.retiredDefinitions.toLocaleString()}</dd>
            </div>
            <div>
              <dt>Stored total</dt>
              <dd>{status.totalDefinitions.toLocaleString()}</dd>
            </div>
            <div>
              <dt>Latest applied release</dt>
              <dd>
                {status.latestReleaseAppliedAt
                  ? new Date(status.latestReleaseAppliedAt).toLocaleString()
                  : "Not yet recorded"}
              </dd>
            </div>
          </dl>
        ) : null}
        {error ? (
          <p className="inline-error" role="alert">
            {error}
          </p>
        ) : null}
        <button
          className="quiet-button"
          disabled={loading}
          onClick={() => setRefreshKey((value) => value + 1)}
          type="button"
        >
          {loading ? "Checking…" : "Refresh status"}
        </button>
        <p className="muted">
          Refresh status reloads diagnostics only. The server handles database
          updates on its configured synchronization interval.
        </p>
      </div>
    </section>
  );
}
