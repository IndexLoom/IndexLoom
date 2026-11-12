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

import { EMPTY_CONNECTOR, type ConnectorDraft } from "./shared";
export function FlareSolverrPanel({
  connectors,
  token,
  onUpdate,
  onDelete,
  onUnauthorized,
}: {
  connectors: FlareSolverrInstance[];
  token: string;
  onUpdate: (connector: FlareSolverrInstance) => void;
  onDelete: (id: string) => void;
  onUnauthorized: () => void;
}) {
  const [draft, setDraft] = useState<ConnectorDraft>(() => ({
    ...EMPTY_CONNECTOR,
  }));
  const [editing, setEditing] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  function edit(connector: FlareSolverrInstance) {
    setDraft({
      id: connector.id,
      name: connector.name,
      endpoint: connector.endpoint,
      enabled: connector.enabled,
      timeoutSeconds: connector.timeoutSeconds,
    });
    setEditing(true);
    setStatus(null);
  }

  function reset() {
    setDraft({ ...EMPTY_CONNECTOR });
    setEditing(false);
    setStatus(null);
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSaving(true);
    setStatus(null);
    try {
      const connector = await saveFlareSolverr(token, draft.id, {
        name: draft.name,
        endpoint: draft.endpoint,
        enabled: draft.enabled,
        timeoutSeconds: draft.timeoutSeconds,
      });
      onUpdate(connector);
      reset();
      setStatus("FlareSolverr connector saved.");
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) {
        onUnauthorized();
        return;
      }
      setStatus(
        error instanceof Error ? error.message : "connector save failed",
      );
    } finally {
      setSaving(false);
    }
  }

  async function remove(id: string) {
    if (
      !window.confirm(
        t("Delete this FlareSolverr connector? Assigned indexers will return to direct mode."),
      )
    ) {
      return;
    }
    setStatus(null);
    try {
      await removeFlareSolverr(token, id);
      onDelete(id);
      if (draft.id === id) {
        reset();
      }
      setStatus("FlareSolverr connector deleted.");
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) {
        onUnauthorized();
        return;
      }
      setStatus(
        error instanceof Error ? error.message : "connector delete failed",
      );
    }
  }

  return (
    <section
      className="configuration-section"
      aria-labelledby="flaresolverr-heading"
    >
      <div className="section-heading stacked-heading">
        <div>
          <h2 id="flaresolverr-heading">{t("FlareSolverr connectors")}</h2>
          <p>{t("Use a browser solver only for definitions that need challenge handling.")}{" "}</p>
        </div>
        <span className="muted">{formatNumber(connectors.length)} {t("configured")}</span>
      </div>
      <div className="connector-layout">
        <div className="connector-list">
          {connectors.length === 0 ? (
            <p className="panel-empty">{t("No connectors configured.")}</p>
          ) : null}
          {connectors.map((connector) => (
            <article className="connector-row" key={connector.id}>
              <div>
                <strong>{connector.name}</strong>
                <span>{connector.endpoint}</span>
              </div>
              <span
                className={
                  connector.enabled ? "status-pill healthy" : "status-pill"
                }
              >
                {t(connector.enabled ? "enabled" : "disabled")}
              </span>
              <button
                className="text-button"
                onClick={() => edit(connector)}
                type="button"
              >{t("Edit")}{" "}</button>
              <button
                className="text-button danger"
                onClick={() => void remove(connector.id)}
                type="button"
              >{t("Delete")}{" "}</button>
            </article>
          ))}
        </div>
        <form className="connector-form" onSubmit={submit}>
          <label>{t("Connector ID")}{" "}<input
              disabled={editing}
              onChange={(event) =>
                setDraft((current) => ({ ...current, id: event.target.value }))
              }
              pattern="[a-z0-9]+(?:-[a-z0-9]+)*"
              placeholder="local"
              required
              value={draft.id}
            />
          </label>
          <label>{t("Name")}{" "}<input
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  name: event.target.value,
                }))
              }
              placeholder={t("Local FlareSolverr")}
              required
              value={draft.name}
            />
          </label>
          <label className="wide-field">{t("Endpoint")}{" "}<input
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  endpoint: event.target.value,
                }))
              }
              required
              type="url"
              value={draft.endpoint}
            />
          </label>
          <label>{t("Timeout, seconds")}{" "}<input
              max={300}
              min={5}
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  timeoutSeconds: event.target.valueAsNumber,
                }))
              }
              required
              type="number"
              value={draft.timeoutSeconds}
            />
          </label>
          <label className="checkbox-field">
            <input
              checked={draft.enabled}
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  enabled: event.target.checked,
                }))
              }
              type="checkbox"
            />{t("Enabled")}{" "}</label>
          <div className="form-actions wide-field">
            <button className="primary-button" disabled={saving} type="submit">
              {saving
                ? "Saving…"
                : editing
                  ? "Update connector"
                  : "Add connector"}
            </button>
            {editing ? (
              <button className="quiet-button" onClick={reset} type="button">{t("Cancel")}{" "}</button>
            ) : null}
          </div>
          {status ? (
            <p className="form-status wide-field" role="status">
              {t(status)}
            </p>
          ) : null}
        </form>
      </div>
    </section>
  );
}

