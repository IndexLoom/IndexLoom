import { type FormEvent, useCallback, useEffect, useState } from "react";

import {
  ApiError,
  type ConfigurationField,
  type ConfigurationValue,
  type FlareSolverrInstance,
  type FlareSolverrUpdate,
  type Indexer,
  fetchFlareSolverr,
  fetchIndexers,
  removeFlareSolverr,
  saveFlareSolverr,
  saveIndexerConfiguration,
} from "../lib/api";

const ADMIN_SESSION_KEY = "indexloom.admin-token.v1";
const EMPTY_CONNECTOR: ConnectorDraft = {
  id: "",
  name: "",
  endpoint: "http://flaresolverr:8191/v1",
  enabled: true,
  timeoutSeconds: 60,
};

type ConnectorDraft = FlareSolverrUpdate & { id: string };

type ManagerState =
  | { kind: "loading" }
  | { kind: "error"; message: string }
  | {
      kind: "ready";
      indexers: Indexer[];
      connectors: FlareSolverrInstance[];
    };

export function IndexersWorkspace() {
  const [token, setToken] = useState(() => sessionStorage.getItem(ADMIN_SESSION_KEY) ?? "");

  const acceptToken = useCallback((value: string) => {
    sessionStorage.setItem(ADMIN_SESSION_KEY, value);
    setToken(value);
  }, []);

  const clearToken = useCallback(() => {
    sessionStorage.removeItem(ADMIN_SESSION_KEY);
    setToken("");
  }, []);

  if (!token) {
    return <AdminTokenGate onSubmit={acceptToken} />;
  }
  return <IndexersManager onUnauthorized={clearToken} token={token} />;
}

function AdminTokenGate({ onSubmit }: { onSubmit: (token: string) => void }) {
  const [value, setValue] = useState("");

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (value.length >= 32) {
      onSubmit(value);
    }
  }

  return (
    <section className="empty-view credential-gate" aria-labelledby="credential-heading">
      <p className="eyebrow">Protected configuration</p>
      <h1 id="credential-heading">Administrator token</h1>
      <p>
        Tracker cookies, API keys, and passwords are managed through an authenticated API.
        The token is kept only in this browser tab session.
      </p>
      <form className="credential-gate-form" onSubmit={submit}>
        <label htmlFor="admin-token">INDEXLOOM_ADMIN_TOKEN</label>
        <input
          autoComplete="current-password"
          id="admin-token"
          minLength={32}
          onChange={(event) => setValue(event.target.value)}
          required
          type="password"
          value={value}
        />
        <button className="primary-button" type="submit">Unlock</button>
      </form>
    </section>
  );
}

function IndexersManager({
  token,
  onUnauthorized,
}: {
  token: string;
  onUnauthorized: () => void;
}) {
  const [state, setState] = useState<ManagerState>({ kind: "loading" });

  useEffect(() => {
    const controller = new AbortController();
    Promise.all([
      fetchIndexers(token, controller.signal),
      fetchFlareSolverr(token, controller.signal),
    ])
      .then(([indexers, connectors]) => {
        setState({ kind: "ready", indexers, connectors });
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) {
          return;
        }
        if (error instanceof ApiError && error.status === 401) {
          onUnauthorized();
          return;
        }
        setState({
          kind: "error",
          message: error instanceof Error ? error.message : "configuration request failed",
        });
      });
    return () => controller.abort();
  }, [onUnauthorized, token]);

  if (state.kind === "loading") {
    return <WorkspaceMessage title="Loading indexers" body="Reading redacted configuration state." />;
  }
  if (state.kind === "error") {
    return <WorkspaceMessage title="Configuration unavailable" body={state.message} />;
  }

  function updateConnector(connector: FlareSolverrInstance) {
    setState((current) => {
      if (current.kind !== "ready") {
        return current;
      }
      const connectors = current.connectors.filter((item) => item.id !== connector.id);
      connectors.push(connector);
      connectors.sort((left, right) => left.name.localeCompare(right.name));
      return { ...current, connectors };
    });
  }

  function deleteConnector(id: string) {
    setState((current) =>
      current.kind === "ready"
        ? {
            ...current,
            connectors: current.connectors.filter((connector) => connector.id !== id),
            indexers: current.indexers.map((indexer) =>
              indexer.flaresolverrInstanceId === id
                ? { ...indexer, flaresolverrInstanceId: null }
                : indexer,
            ),
          }
        : current,
    );
  }

  function updateIndexer(indexer: Indexer) {
    setState((current) =>
      current.kind === "ready"
        ? {
            ...current,
            indexers: current.indexers.map((item) => (item.id === indexer.id ? indexer : item)),
          }
        : current,
    );
  }

  return (
    <section className="settings-view" aria-labelledby="indexers-heading">
      <div className="page-heading">
        <div>
          <p className="eyebrow">Network identity and credentials</p>
          <h1 id="indexers-heading">Indexers</h1>
        </div>
        <button className="quiet-button" onClick={onUnauthorized} type="button">Lock</button>
      </div>

      <FlareSolverrPanel
        connectors={state.connectors}
        onDelete={deleteConnector}
        onUnauthorized={onUnauthorized}
        onUpdate={updateConnector}
        token={token}
      />

      <section className="configuration-section" aria-labelledby="tracker-config-heading">
        <div className="section-heading stacked-heading">
          <div>
            <h2 id="tracker-config-heading">Tracker configurations</h2>
            <p>Secret values are encrypted at rest and the API returns only configured/not configured state.</p>
          </div>
          <span className="muted">{state.indexers.length} configured</span>
        </div>
        {state.indexers.length === 0 ? (
          <div className="panel-empty">
            No indexer instances are installed yet. Configuration controls will appear here after a definition is installed.
          </div>
        ) : (
          <div className="indexer-grid">
            {state.indexers.map((indexer) => (
              <IndexerEditor
                connectors={state.connectors}
                indexer={indexer}
                key={indexer.id}
                onSaved={updateIndexer}
                onUnauthorized={onUnauthorized}
                token={token}
              />
            ))}
          </div>
        )}
      </section>
    </section>
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

function FlareSolverrPanel({
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
  const [draft, setDraft] = useState<ConnectorDraft>(() => ({ ...EMPTY_CONNECTOR }));
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
      setStatus(error instanceof Error ? error.message : "connector save failed");
    } finally {
      setSaving(false);
    }
  }

  async function remove(id: string) {
    if (!window.confirm("Delete this FlareSolverr connector? Assigned indexers will return to direct mode.")) {
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
      setStatus(error instanceof Error ? error.message : "connector delete failed");
    }
  }

  return (
    <section className="configuration-section" aria-labelledby="flaresolverr-heading">
      <div className="section-heading stacked-heading">
        <div>
          <h2 id="flaresolverr-heading">FlareSolverr connectors</h2>
          <p>Use a browser solver only for definitions that need challenge handling.</p>
        </div>
        <span className="muted">{connectors.length} configured</span>
      </div>
      <div className="connector-layout">
        <div className="connector-list">
          {connectors.length === 0 ? <p className="panel-empty">No connectors configured.</p> : null}
          {connectors.map((connector) => (
            <article className="connector-row" key={connector.id}>
              <div>
                <strong>{connector.name}</strong>
                <span>{connector.endpoint}</span>
              </div>
              <span className={connector.enabled ? "status-pill healthy" : "status-pill"}>
                {connector.enabled ? "enabled" : "disabled"}
              </span>
              <button className="text-button" onClick={() => edit(connector)} type="button">Edit</button>
              <button className="text-button danger" onClick={() => void remove(connector.id)} type="button">Delete</button>
            </article>
          ))}
        </div>
        <form className="connector-form" onSubmit={submit}>
          <label>
            Connector ID
            <input
              disabled={editing}
              onChange={(event) => setDraft((current) => ({ ...current, id: event.target.value }))}
              pattern="[a-z0-9]+(?:-[a-z0-9]+)*"
              placeholder="local"
              required
              value={draft.id}
            />
          </label>
          <label>
            Name
            <input
              onChange={(event) => setDraft((current) => ({ ...current, name: event.target.value }))}
              placeholder="Local FlareSolverr"
              required
              value={draft.name}
            />
          </label>
          <label className="wide-field">
            Endpoint
            <input
              onChange={(event) => setDraft((current) => ({ ...current, endpoint: event.target.value }))}
              required
              type="url"
              value={draft.endpoint}
            />
          </label>
          <label>
            Timeout, seconds
            <input
              max={300}
              min={5}
              onChange={(event) => setDraft((current) => ({ ...current, timeoutSeconds: event.target.valueAsNumber }))}
              required
              type="number"
              value={draft.timeoutSeconds}
            />
          </label>
          <label className="checkbox-field">
            <input
              checked={draft.enabled}
              onChange={(event) => setDraft((current) => ({ ...current, enabled: event.target.checked }))}
              type="checkbox"
            />
            Enabled
          </label>
          <div className="form-actions wide-field">
            <button className="primary-button" disabled={saving} type="submit">
              {saving ? "Saving…" : editing ? "Update connector" : "Add connector"}
            </button>
            {editing ? <button className="quiet-button" onClick={reset} type="button">Cancel</button> : null}
          </div>
          {status ? <p className="form-status wide-field" role="status">{status}</p> : null}
        </form>
      </div>
    </section>
  );
}

function IndexerEditor({
  indexer,
  connectors,
  token,
  onSaved,
  onUnauthorized,
}: {
  indexer: Indexer;
  connectors: FlareSolverrInstance[];
  token: string;
  onSaved: (indexer: Indexer) => void;
  onUnauthorized: () => void;
}) {
  const [values, setValues] = useState<Record<string, ConfigurationValue>>(() => initialValues(indexer));
  const [userAgent, setUserAgent] = useState(indexer.userAgent ?? "");
  const [connectorId, setConnectorId] = useState(indexer.flaresolverrInstanceId ?? "");
  const [secretInputs, setSecretInputs] = useState<Record<string, string>>({});
  const [clearSecrets, setClearSecrets] = useState<Set<string>>(() => new Set());
  const [status, setStatus] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const configured = new Set(indexer.configuredSecrets);

  function setValue(field: ConfigurationField, value: ConfigurationValue) {
    setValues((current) => ({ ...current, [field.name]: value }));
  }

  function toggleClear(name: string, clear: boolean) {
    setClearSecrets((current) => {
      const next = new Set(current);
      if (clear) {
        next.add(name);
      } else {
        next.delete(name);
      }
      return next;
    });
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSaving(true);
    setStatus(null);
    const secrets: Record<string, string | null> = {};
    for (const field of indexer.configurationSchema.fields) {
      if (!field.secret) {
        continue;
      }
      if (clearSecrets.has(field.name)) {
        secrets[field.name] = null;
      } else {
        const input = secretInputs[field.name];
        if (input) {
          secrets[field.name] = input;
        }
      }
    }
    try {
      const saved = await saveIndexerConfiguration(token, indexer.id, {
        userAgent: userAgent.trim() || null,
        flaresolverrInstanceId: connectorId || null,
        values,
        secrets,
      });
      onSaved(saved);
      setSecretInputs({});
      setClearSecrets(new Set());
      setStatus("Configuration saved. Secret values remain redacted.");
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) {
        onUnauthorized();
        return;
      }
      setStatus(error instanceof Error ? error.message : "configuration save failed");
    } finally {
      setSaving(false);
    }
  }

  return (
    <form className="indexer-card" onSubmit={submit}>
      <header>
        <div>
          <h3>{indexer.displayName}</h3>
          <p>{indexer.definitionId}</p>
        </div>
        <span className={`privacy-pill ${indexer.privacy}`}>{indexer.privacy}</span>
      </header>
      {indexer.configurationSchema.flaresolverrRecommended ? (
        <p className="recommendation">This definition recommends FlareSolverr challenge handling.</p>
      ) : null}
      <div className="field-grid">
        <label className="wide-field">
          User-Agent override
          <input
            onChange={(event) => setUserAgent(event.target.value)}
            placeholder="Inherit the IndexLoom default"
            value={userAgent}
          />
        </label>
        <label className="wide-field">
          Request route
          <select onChange={(event) => setConnectorId(event.target.value)} value={connectorId}>
            <option value="">Direct HTTP</option>
            {connectors.filter((connector) => connector.enabled).map((connector) => (
              <option key={connector.id} value={connector.id}>{connector.name}</option>
            ))}
          </select>
        </label>
        {indexer.configurationSchema.fields.map((field) =>
          field.secret ? (
            <SecretField
              clear={clearSecrets.has(field.name)}
              configured={configured.has(field.name)}
              field={field}
              key={field.name}
              onClear={toggleClear}
              onValue={(name, value) => setSecretInputs((current) => ({ ...current, [name]: value }))}
              value={secretInputs[field.name] ?? ""}
            />
          ) : (
            <PublicField
              field={field}
              key={field.name}
              onChange={setValue}
              value={values[field.name]}
            />
          ),
        )}
      </div>
      <div className="form-actions">
        <button className="primary-button" disabled={saving} type="submit">
          {saving ? "Saving…" : "Save configuration"}
        </button>
        {status ? <p className="form-status" role="status">{status}</p> : null}
      </div>
    </form>
  );
}

function PublicField({
  field,
  value,
  onChange,
}: {
  field: ConfigurationField;
  value: ConfigurationValue | undefined;
  onChange: (field: ConfigurationField, value: ConfigurationValue) => void;
}) {
  if (field.kind === "checkbox") {
    return (
      <label className="checkbox-field wide-field">
        <input
          checked={value === true}
          onChange={(event) => onChange(field, event.target.checked)}
          type="checkbox"
        />
        {field.label}
      </label>
    );
  }
  if (field.kind === "select") {
    return (
      <label>
        {field.label}
        <select
          onChange={(event) => onChange(field, event.target.value)}
          value={value === undefined ? "" : String(value)}
        >
          {field.options.map((option) => (
            <option key={option.value} value={option.value}>{option.label}</option>
          ))}
        </select>
      </label>
    );
  }
  return (
    <label>
      {field.label}
      <input
        onChange={(event) => onChange(field, event.target.value)}
        value={value === undefined ? "" : String(value)}
      />
    </label>
  );
}

function SecretField({
  field,
  configured,
  clear,
  value,
  onValue,
  onClear,
}: {
  field: ConfigurationField;
  configured: boolean;
  clear: boolean;
  value: string;
  onValue: (name: string, value: string) => void;
  onClear: (name: string, clear: boolean) => void;
}) {
  return (
    <div className="secret-field wide-field">
      <label>
        <span>
          {field.label}
          <small className={configured ? "secret-state configured" : "secret-state"}>
            {configured ? "configured" : "not configured"}
          </small>
        </span>
        <input
          autoComplete="new-password"
          disabled={clear}
          onChange={(event) => onValue(field.name, event.target.value)}
          placeholder={configured ? "Leave blank to keep current value" : "Enter secret value"}
          type="password"
          value={value}
        />
      </label>
      {configured ? (
        <label className="checkbox-field clear-secret">
          <input
            checked={clear}
            onChange={(event) => onClear(field.name, event.target.checked)}
            type="checkbox"
          />
          Clear saved value
        </label>
      ) : null}
    </div>
  );
}

function initialValues(indexer: Indexer): Record<string, ConfigurationValue> {
  const values = { ...indexer.values };
  for (const field of indexer.configurationSchema.fields) {
    if (field.secret || values[field.name] !== undefined) {
      continue;
    }
    if (field.defaultValue !== undefined) {
      values[field.name] = field.defaultValue;
    } else if (field.kind === "checkbox") {
      values[field.name] = false;
    } else if (field.kind === "select" && field.options[0]) {
      values[field.name] = field.options[0].value;
    }
  }
  return values;
}
