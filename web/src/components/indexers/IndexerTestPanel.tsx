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

export function IndexerTestPanel({
  indexers,
  groups,
  token,
  onUnauthorized,
}: {
  indexers: Indexer[];
  groups: IndexerGroup[];
  token: string;
  onUnauthorized: () => void;
}) {
  const [selection, setSelection] = useState<IndexerSelection>({
    all: true,
    indexerIds: [],
    groupIds: [],
  });
  const [job, setJob] = useState<IndexerTestJob | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    if (!job || job.status === "completed") return;
    let active = true;
    const timer = window.setInterval(() => {
      fetchIndexerTestJob(token, job.id)
        .then((next) => {
          if (active) setJob(next);
        })
        .catch((error: unknown) => {
          if (!active) return;
          if (error instanceof ApiError && error.status === 401)
            onUnauthorized();
          else
            setStatus(
              error instanceof Error
                ? error.message
                : "test status request failed",
            );
        });
    }, 1000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [job, onUnauthorized, token]);

  function toggle(
    kind: "indexerIds" | "groupIds",
    id: string,
    checked: boolean,
  ) {
    const values = new Set(selection[kind]);
    if (checked) values.add(id);
    else values.delete(id);
    setSelection({
      ...selection,
      all: false,
      [kind]: Array.from(values).sort(),
    });
  }

  async function start(chosen: IndexerSelection) {
    setStatus(null);
    try {
      setJob(await startIndexerTests(token, chosen));
    } catch (error) {
      if (error instanceof ApiError && error.status === 401)
        return onUnauthorized();
      setStatus(error instanceof Error ? error.message : "test start failed");
    }
  }

  return (
    <section className="configuration-section" aria-labelledby="test-heading">
      <div className="section-heading stacked-heading">
        <div>
          <h2 id="test-heading">{t("Indexer tests")}</h2>
          <p>{t("Test every enabled indexer, selected groups, or individual definitions with bounded parallelism.")}{" "}</p>
        </div>
        <span className="muted">
          {job ? `${formatNumber(job.completed)}/${formatNumber(job.total)} ${t(job.status)}` : t("idle")}
        </span>
      </div>
      <div className="test-panel">
        <div className="bulk-actions">
          <button
            className="primary-button"
            disabled={job?.status === "running"}
            onClick={() =>
              void start({ all: true, indexerIds: [], groupIds: [] })
            }
            type="button"
          >{t("Test all enabled")}{" "}</button>
          <button
            className="quiet-button"
            disabled={
              job?.status === "running" ||
              (selection.indexerIds.length === 0 &&
                selection.groupIds.length === 0)
            }
            onClick={() => void start(selection)}
            type="button"
          >{t("Test selection")}{" "}</button>
        </div>
        <details className="test-selection">
          <summary>{t("Choose groups and indexers")}</summary>
          <div className="test-choice-grid">
            <fieldset>
              <legend>{t("Groups")}</legend>
              {groups.map((group) => (
                <label className="checkbox-field" key={group.id}>
                  <input
                    checked={selection.groupIds.includes(group.id)}
                    onChange={(event) =>
                      toggle("groupIds", group.id, event.target.checked)
                    }
                    type="checkbox"
                  />
                  {group.name} <small>{group.memberCount}</small>
                </label>
              ))}
            </fieldset>
            <fieldset>
              <legend>{t("Indexers")}</legend>
              <div className="test-indexer-list">
                {indexers
                  .filter((indexer) => indexer.enabled && indexer.catalogAvailable)
                  .map((indexer) => (
                    <label className="checkbox-field" key={indexer.id}>
                      <input
                        checked={selection.indexerIds.includes(indexer.id)}
                        onChange={(event) =>
                          toggle("indexerIds", indexer.id, event.target.checked)
                        }
                        type="checkbox"
                      />
                      {indexer.displayName}
                    </label>
                  ))}
              </div>
            </fieldset>
          </div>
        </details>
        {status ? (
          <p className="form-status" role="status">
            {t(status)}
          </p>
        ) : null}
        {job?.results.length ? (
          <div className="table-frame">
            <table>
              <thead>
                <tr>
                  <th>{t("Indexer")}</th>
                  <th>{t("Result")}</th>
                  <th>{t("Response")}</th>
                  <th>{t("Message")}</th>
                </tr>
              </thead>
              <tbody>
                {job.results.map((result) => (
                  <tr key={result.indexerId}>
                    <td>{result.indexerName}</td>
                    <td>
                      <span
                        className={
                          result.success ? "status-pill healthy" : "status-pill"
                        }
                      >
                        {result.success ? "success" : "failed"}
                      </span>
                    </td>
                    <td>{result.responseTimeMs} ms</td>
                    <td>{result.message}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : null}
      </div>
    </section>
  );
}

