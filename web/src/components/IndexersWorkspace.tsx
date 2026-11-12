import { t } from "../lib/i18n";
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
} from "../lib/api";
import { AdminTokenGate, useAdminSession } from "./AdminSession";

const EMPTY_CONNECTOR: ConnectorDraft = {
  id: "",
  name: "",
  endpoint: "http://flaresolverr:8191/v1",
  enabled: true,
  timeoutSeconds: 60,
};

type ConnectorDraft = FlareSolverrUpdate & { id: string };
type ProxyDraft = OutboundProxyUpdate & { id: string };
type GroupDraft = IndexerGroupUpdate & { id: string };

const EMPTY_PROXY: ProxyDraft = {
  id: "",
  name: "",
  kind: "http",
  host: "",
  port: 8080,
  enabled: true,
  username: null,
  clearPassword: false,
};

const EMPTY_GROUP: GroupDraft = {
  id: "",
  name: "",
  description: "",
  enabled: true,
  priority: 0,
  proxyId: null,
};

type ManagerState =
  | { kind: "loading" }
  | { kind: "error"; message: string }
  | {
      kind: "ready";
      indexers: Indexer[];
      connectors: FlareSolverrInstance[];
      proxies: OutboundProxy[];
      groups: IndexerGroup[];
      routing: RoutingSettings;
    };

type ManagementView =
  | "catalog"
  | "tests"
  | "routing"
  | "groups"
  | "flaresolverr";

const managementNavigation: Array<{
  id: ManagementView;
  label: string;
  description: string;
}> = [
  {
    id: "catalog",
    label: "Catalog",
    description: "Enable and configure indexers",
  },
  { id: "tests", label: "Tests", description: "Check selected indexers" },
  {
    id: "routing",
    label: "Routing",
    description: "Global and scoped proxies",
  },
  { id: "groups", label: "Groups", description: "Organize indexers" },
  {
    id: "flaresolverr",
    label: "FlareSolverr",
    description: "Browser challenge connectors",
  },
];

export function IndexersWorkspace() {
  const { token, acceptToken, clearToken } = useAdminSession();

  if (!token) {
    return <AdminTokenGate onSubmit={acceptToken} />;
  }
  return <IndexersManager onUnauthorized={clearToken} token={token} />;
}

function IndexersManager({
  token,
  onUnauthorized,
}: {
  token: string;
  onUnauthorized: () => void;
}) {
  const [state, setState] = useState<ManagerState>({ kind: "loading" });
  const [managementView, setManagementView] =
    useState<ManagementView>("catalog");

  useEffect(() => {
    const controller = new AbortController();
    Promise.all([
      fetchIndexers(token, controller.signal),
      fetchFlareSolverr(token, controller.signal),
      fetchProxies(token, controller.signal),
      fetchGroups(token, controller.signal),
      fetchRouting(token, controller.signal),
    ])
      .then(([indexers, connectors, proxies, groups, routing]) => {
        setState({
          kind: "ready",
          indexers,
          connectors,
          proxies,
          groups,
          routing,
        });
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
          message:
            error instanceof Error
              ? error.message
              : "configuration request failed",
        });
      });
    return () => controller.abort();
  }, [onUnauthorized, token]);

  if (state.kind === "loading") {
    return (
      <WorkspaceMessage
        title={t("Loading indexers")}
        body="Reading redacted configuration state."
      />
    );
  }
  if (state.kind === "error") {
    return (
      <WorkspaceMessage
        title={t("Configuration unavailable")}
        body={state.message}
      />
    );
  }

  function updateConnector(connector: FlareSolverrInstance) {
    setState((current) => {
      if (current.kind !== "ready") {
        return current;
      }
      const connectors = current.connectors.filter(
        (item) => item.id !== connector.id,
      );
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
            connectors: current.connectors.filter(
              (connector) => connector.id !== id,
            ),
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
            indexers: current.indexers.map((item) =>
              item.id === indexer.id ? indexer : item,
            ),
          }
        : current,
    );
  }

  function updateProxy(proxy: OutboundProxy) {
    setState((current) => {
      if (current.kind !== "ready") return current;
      const proxies = current.proxies.filter((item) => item.id !== proxy.id);
      proxies.push(proxy);
      proxies.sort((left, right) => left.name.localeCompare(right.name));
      return { ...current, proxies };
    });
  }

  function deleteProxy(id: string) {
    setState((current) =>
      current.kind === "ready"
        ? {
            ...current,
            proxies: current.proxies.filter((proxy) => proxy.id !== id),
            routing:
              current.routing.globalProxyId === id
                ? { globalProxyId: null }
                : current.routing,
            groups: current.groups.map((group) =>
              group.proxyId === id ? { ...group, proxyId: null } : group,
            ),
            indexers: current.indexers.map((indexer) =>
              indexer.proxyId === id ? { ...indexer, proxyId: null } : indexer,
            ),
          }
        : current,
    );
  }

  function updateGroup(group: IndexerGroup) {
    setState((current) => {
      if (current.kind !== "ready") return current;
      const groups = current.groups.filter((item) => item.id !== group.id);
      groups.push(group);
      groups.sort((left, right) => left.name.localeCompare(right.name));
      return { ...current, groups };
    });
  }

  function deleteGroup(id: string) {
    setState((current) =>
      current.kind === "ready"
        ? {
            ...current,
            groups: current.groups.filter((group) => group.id !== id),
            indexers: current.indexers.map((indexer) => ({
              ...indexer,
              groupIds: indexer.groupIds.filter((groupId) => groupId !== id),
            })),
          }
        : current,
    );
  }

  function updateRouting(routing: RoutingSettings) {
    setState((current) =>
      current.kind === "ready" ? { ...current, routing } : current,
    );
  }

  const enabledCount = state.indexers.filter(
    (indexer) => indexer.enabled,
  ).length;
  const activeProxyCount = state.proxies.filter(
    (proxy) => proxy.enabled,
  ).length;

  return (
    <section className="settings-view" data-layout-kind="indexers" aria-labelledby="indexers-heading">
      <div className="page-heading">
        <div>
          <p className="eyebrow">{t("Indexer control center")}</p>
          <h1 id="indexers-heading">{t("Indexers")}</h1>
          <p className="page-description">{t("Configure sources, routing, credentials, and health checks without leaving this workspace.")}{" "}</p>
        </div>
        <button className="quiet-button" onClick={onUnauthorized} type="button">{t("Lock")}{" "}</button>
      </div>

      <div className="overview-grid" data-ui-section="overview" aria-label={t("Indexer overview")}>
        <OverviewMetric label="Definitions" value={state.indexers.length} />
        <OverviewMetric label="Enabled" tone="healthy" value={enabledCount} />
        <OverviewMetric label="Groups" value={state.groups.length} />
        <OverviewMetric
          label="Active proxies"
          value={activeProxyCount}
        />
      </div>

      <nav className="management-tabs" data-ui-section="tools" aria-label={t("Indexer management")}>
        {managementNavigation.map((item) => (
          <button
            aria-pressed={managementView === item.id}
            className={managementView === item.id ? "active" : ""}
            key={item.id}
            onClick={() => setManagementView(item.id)}
            type="button"
          >
            <strong>{item.label}</strong>
            <span>{item.description}</span>
          </button>
        ))}
      </nav>

      <div className="management-panel" data-ui-section="panel">
        {managementView === "catalog" ? (
          <IndexerConfigurationPanel
            connectors={state.connectors}
            groups={state.groups}
            indexers={state.indexers}
            onSaved={updateIndexer}
            onUnauthorized={onUnauthorized}
            proxies={state.proxies}
            token={token}
          />
        ) : null}

        {managementView === "tests" ? (
          <IndexerTestPanel
            groups={state.groups}
            indexers={state.indexers}
            onUnauthorized={onUnauthorized}
            token={token}
          />
        ) : null}

        {managementView === "routing" ? (
          <ProxyPanel
            onDelete={deleteProxy}
            onRouting={updateRouting}
            onUnauthorized={onUnauthorized}
            onUpdate={updateProxy}
            proxies={state.proxies}
            routing={state.routing}
            token={token}
          />
        ) : null}

        {managementView === "groups" ? (
          <GroupPanel
            groups={state.groups}
            onDelete={deleteGroup}
            onUnauthorized={onUnauthorized}
            onUpdate={updateGroup}
            proxies={state.proxies}
            token={token}
          />
        ) : null}

        {managementView === "flaresolverr" ? (
          <FlareSolverrPanel
            connectors={state.connectors}
            onDelete={deleteConnector}
            onUnauthorized={onUnauthorized}
            onUpdate={updateConnector}
            token={token}
          />
        ) : null}
      </div>
    </section>
  );
}

function OverviewMetric({
  label,
  value,
  tone = "neutral",
}: {
  label: string;
  value: number;
  tone?: "neutral" | "healthy";
}) {
  return (
    <div className={`overview-metric ${tone}`}>
      <strong>{value.toLocaleString()}</strong>
      <span>{label}</span>
    </div>
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

function ProxyPanel({
  proxies,
  routing,
  token,
  onUpdate,
  onDelete,
  onRouting,
  onUnauthorized,
}: {
  proxies: OutboundProxy[];
  routing: RoutingSettings;
  token: string;
  onUpdate: (proxy: OutboundProxy) => void;
  onDelete: (id: string) => void;
  onRouting: (routing: RoutingSettings) => void;
  onUnauthorized: () => void;
}) {
  const [draft, setDraft] = useState<ProxyDraft>({ ...EMPTY_PROXY });
  const [editing, setEditing] = useState(false);
  const [password, setPassword] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  function reset() {
    setDraft({ ...EMPTY_PROXY });
    setPassword("");
    setEditing(false);
  }

  function edit(proxy: OutboundProxy) {
    setDraft({
      id: proxy.id,
      name: proxy.name,
      kind: proxy.kind,
      host: proxy.host,
      port: proxy.port,
      enabled: proxy.enabled,
      username: proxy.username,
      clearPassword: false,
    });
    setPassword("");
    setEditing(true);
    setStatus(null);
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSaving(true);
    setStatus(null);
    try {
      const update: OutboundProxyUpdate = {
        name: draft.name,
        kind: draft.kind,
        host: draft.host,
        port: draft.port,
        enabled: draft.enabled,
        username: draft.username?.trim() || null,
        clearPassword: draft.clearPassword,
      };
      if (password) update.password = password;
      const proxy = await saveProxy(token, draft.id, update);
      onUpdate(proxy);
      reset();
      setStatus("Outbound proxy saved.");
    } catch (error) {
      if (error instanceof ApiError && error.status === 401)
        return onUnauthorized();
      setStatus(error instanceof Error ? error.message : "proxy save failed");
    } finally {
      setSaving(false);
    }
  }

  async function remove(id: string) {
    if (
      !window.confirm(
        "Delete this outbound proxy? Global, group and indexer assignments will fall back automatically.",
      )
    )
      return;
    try {
      await removeProxy(token, id);
      onDelete(id);
      if (draft.id === id) reset();
      setStatus("Outbound proxy deleted.");
    } catch (error) {
      if (error instanceof ApiError && error.status === 401)
        return onUnauthorized();
      setStatus(error instanceof Error ? error.message : "proxy delete failed");
    }
  }

  async function changeGlobal(value: string) {
    try {
      onRouting(await saveRouting(token, { globalProxyId: value || null }));
      setStatus(
        value ? "Global proxy route saved." : "Global direct route saved.",
      );
    } catch (error) {
      if (error instanceof ApiError && error.status === 401)
        return onUnauthorized();
      setStatus(error instanceof Error ? error.message : "routing save failed");
    }
  }

  return (
    <section className="configuration-section" aria-labelledby="proxy-heading">
      <div className="section-heading stacked-heading">
        <div>
          <h2 id="proxy-heading">{t("Outbound proxies")}</h2>
          <p>{t("HTTP, SOCKS4a and SOCKS5h routes can be assigned globally, by group, or to one indexer.")}{" "}</p>
        </div>
        <label className="inline-select">{t("Global route")}{" "}<select
            onChange={(event) => void changeGlobal(event.target.value)}
            value={routing.globalProxyId ?? ""}
          >
            <option value="">{t("Direct")}</option>
            {proxies
              .filter((proxy) => proxy.enabled)
              .map((proxy) => (
                <option key={proxy.id} value={proxy.id}>
                  {proxy.name}
                </option>
              ))}
          </select>
        </label>
      </div>
      <div className="connector-layout">
        <div className="connector-list">
          {proxies.length === 0 ? (
            <p className="panel-empty">{t("No outbound proxies configured.")}</p>
          ) : null}
          {proxies.map((proxy) => (
            <article className="connector-row" key={proxy.id}>
              <div>
                <strong>{proxy.name}</strong>
                <span>
                  {proxy.kind} · {proxy.host}:{proxy.port}
                  {proxy.passwordConfigured ? " · password saved" : ""}
                </span>
              </div>
              <span
                className={
                  proxy.enabled ? "status-pill healthy" : "status-pill"
                }
              >
                {proxy.enabled ? "enabled" : "disabled"}
              </span>
              <button
                className="text-button"
                onClick={() => edit(proxy)}
                type="button"
              >{t("Edit")}{" "}</button>
              <button
                className="text-button danger"
                onClick={() => void remove(proxy.id)}
                type="button"
              >{t("Delete")}{" "}</button>
            </article>
          ))}
        </div>
        <form className="connector-form" onSubmit={submit}>
          <label>{t("Proxy ID")}{" "}<input
              disabled={editing}
              onChange={(event) =>
                setDraft((current) => ({ ...current, id: event.target.value }))
              }
              pattern="[a-z0-9]+(?:-[a-z0-9]+)*"
              placeholder="tracker-proxy"
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
              required
              value={draft.name}
            />
          </label>
          <label>{t("Protocol")}{" "}<select
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  kind: event.target.value as ProxyKind,
                }))
              }
              value={draft.kind}
            >
              <option value="http">HTTP</option>
              <option value="socks4">SOCKS4a</option>
              <option value="socks5">SOCKS5h</option>
            </select>
          </label>
          <label>{t("Port")}{" "}<input
              max={65535}
              min={1}
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  port: event.target.valueAsNumber,
                }))
              }
              required
              type="number"
              value={draft.port}
            />
          </label>
          <label className="wide-field">{t("Host")}{" "}<input
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  host: event.target.value,
                }))
              }
              placeholder="proxy.example.net"
              required
              value={draft.host}
            />
          </label>
          <label>{t("Username")}{" "}<input
              autoComplete="off"
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  username: event.target.value,
                }))
              }
              value={draft.username ?? ""}
            />
          </label>
          <label>{t("Password")}{" "}<input
              autoComplete="new-password"
              disabled={draft.clearPassword}
              onChange={(event) => setPassword(event.target.value)}
              placeholder={editing ? "Leave blank to preserve" : "Optional"}
              type="password"
              value={password}
            />
          </label>
          {editing ? (
            <label className="checkbox-field wide-field">
              <input
                checked={draft.clearPassword}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    clearPassword: event.target.checked,
                  }))
                }
                type="checkbox"
              />{t("Clear saved password")}{" "}</label>
          ) : null}
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
              {saving ? "Saving…" : editing ? "Update proxy" : "Add proxy"}
            </button>
            {editing ? (
              <button className="quiet-button" onClick={reset} type="button">{t("Cancel")}{" "}</button>
            ) : null}
          </div>
          {status ? (
            <p className="form-status wide-field" role="status">
              {status}
            </p>
          ) : null}
        </form>
      </div>
    </section>
  );
}

function GroupPanel({
  groups,
  proxies,
  token,
  onUpdate,
  onDelete,
  onUnauthorized,
}: {
  groups: IndexerGroup[];
  proxies: OutboundProxy[];
  token: string;
  onUpdate: (group: IndexerGroup) => void;
  onDelete: (id: string) => void;
  onUnauthorized: () => void;
}) {
  const [draft, setDraft] = useState<GroupDraft>({ ...EMPTY_GROUP });
  const [editing, setEditing] = useState(false);
  const [status, setStatus] = useState<string | null>(null);

  function reset() {
    setDraft({ ...EMPTY_GROUP });
    setEditing(false);
  }
  function edit(group: IndexerGroup) {
    setDraft({
      id: group.id,
      name: group.name,
      description: group.description,
      enabled: group.enabled,
      priority: group.priority,
      proxyId: group.proxyId,
    });
    setEditing(true);
    setStatus(null);
  }
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    try {
      onUpdate(
        await saveGroup(token, draft.id, {
          name: draft.name,
          description: draft.description,
          enabled: draft.enabled,
          priority: draft.priority,
          proxyId: draft.proxyId || null,
        }),
      );
      reset();
      setStatus("Indexer group saved.");
    } catch (error) {
      if (error instanceof ApiError && error.status === 401)
        return onUnauthorized();
      setStatus(error instanceof Error ? error.message : "group save failed");
    }
  }
  async function remove(id: string) {
    if (
      !window.confirm(
        "Delete this group? Indexers stay installed and only membership is removed.",
      )
    )
      return;
    try {
      await removeGroup(token, id);
      onDelete(id);
      if (draft.id === id) reset();
      setStatus("Indexer group deleted.");
    } catch (error) {
      if (error instanceof ApiError && error.status === 401)
        return onUnauthorized();
      setStatus(error instanceof Error ? error.message : "group delete failed");
    }
  }
  return (
    <section className="configuration-section" aria-labelledby="group-heading">
      <div className="section-heading stacked-heading">
        <div>
          <h2 id="group-heading">{t("Indexer groups")}</h2>
          <p>{t("One indexer can belong to several groups. Higher-priority enabled groups provide the inherited proxy.")}{" "}</p>
        </div>
        <span className="muted">{groups.length} groups</span>
      </div>
      <div className="connector-layout">
        <div className="connector-list">
          {groups.length === 0 ? (
            <p className="panel-empty">{t("No groups configured.")}</p>
          ) : null}
          {groups.map((group) => (
            <article className="connector-row" key={group.id}>
              <div>
                <strong>{group.name}</strong>
                <span>
                  {group.memberCount} members · priority {group.priority}
                  {group.proxyId ? ` · proxy ${group.proxyId}` : ""}
                </span>
              </div>
              <span
                className={
                  group.enabled ? "status-pill healthy" : "status-pill"
                }
              >
                {group.enabled ? "enabled" : "disabled"}
              </span>
              <button
                className="text-button"
                onClick={() => edit(group)}
                type="button"
              >{t("Edit")}{" "}</button>
              <button
                className="text-button danger"
                onClick={() => void remove(group.id)}
                type="button"
              >{t("Delete")}{" "}</button>
            </article>
          ))}
        </div>
        <form className="connector-form" onSubmit={submit}>
          <label>{t("Group ID")}{" "}<input
              disabled={editing}
              onChange={(event) =>
                setDraft((current) => ({ ...current, id: event.target.value }))
              }
              pattern="[a-z0-9]+(?:-[a-z0-9]+)*"
              placeholder="anime"
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
              required
              value={draft.name}
            />
          </label>
          <label>{t("Priority")}{" "}<input
              max={10000}
              min={-10000}
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  priority: event.target.valueAsNumber,
                }))
              }
              required
              type="number"
              value={draft.priority}
            />
          </label>
          <label>{t("Inherited proxy")}{" "}<select
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  proxyId: event.target.value || null,
                }))
              }
              value={draft.proxyId ?? ""}
            >
              <option value="">{t("Inherit global")}</option>
              {proxies
                .filter((proxy) => proxy.enabled)
                .map((proxy) => (
                  <option key={proxy.id} value={proxy.id}>
                    {proxy.name}
                  </option>
                ))}
            </select>
          </label>
          <label className="wide-field">{t("Description")}{" "}<input
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  description: event.target.value,
                }))
              }
              value={draft.description}
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
            <button className="primary-button" type="submit">
              {editing ? "Update group" : "Add group"}
            </button>
            {editing ? (
              <button className="quiet-button" onClick={reset} type="button">{t("Cancel")}{" "}</button>
            ) : null}
          </div>
          {status ? (
            <p className="form-status wide-field" role="status">
              {status}
            </p>
          ) : null}
        </form>
      </div>
    </section>
  );
}

function IndexerTestPanel({
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
          {job ? `${job.completed}/${job.total} ${job.status}` : "idle"}
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
            {status}
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

function IndexerConfigurationPanel({
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
          Showing {filtered.length} of {indexers.length}
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
            {languages.map((language) => <option key={language} value={language}>{language}</option>)}
          </select>
        </label>
        <label className="catalog-filter">
          <span>{t("Category")}</span>
          <select value={categoryFilter} onChange={(event) => { setCategoryFilter(event.target.value); setLimit(60); }}>
            <option value="all">{t("All categories")}</option>
            {categories.map((category) => <option key={category} value={category}>{category}</option>)}
          </select>
        </label>
        <label className="catalog-filter">
          <span>{t("Search mode")}</span>
          <select value={modeFilter} onChange={(event) => { setModeFilter(event.target.value); setLimit(60); }}>
            <option value="all">{t("Any mode")}</option>
            {modes.map((mode) => <option key={mode} value={mode}>{mode}</option>)}
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
                  <small>{indexer.definitionId} · {indexer.capabilities.language} · {indexer.capabilities.source === "jackett" ? "Jackett" : "Prowlarr"}</small>
                </span>
                <span className="indexer-list-meta">
                  <span className={`privacy-pill ${indexer.privacy}`}>
                    {indexer.privacy}
                  </span>
                  <span
                    className={
                      indexer.catalogAvailable && indexer.enabled
                        ? "status-pill healthy"
                        : "status-pill"
                    }
                  >
                    {!indexer.catalogAvailable
                      ? "unavailable"
                      : indexer.enabled
                        ? "enabled"
                        : "disabled"}
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
                  Show {Math.min(60, filtered.length - visible.length)} more
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
        "Delete this FlareSolverr connector? Assigned indexers will return to direct mode.",
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
        <span className="muted">{connectors.length} configured</span>
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
                {connector.enabled ? "enabled" : "disabled"}
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
              {status}
            </p>
          ) : null}
        </form>
      </div>
    </section>
  );
}

function IndexerEditor({
  indexer,
  connectors,
  proxies,
  groups,
  token,
  onClose,
  onSaved,
  onUnauthorized,
}: {
  indexer: Indexer;
  connectors: FlareSolverrInstance[];
  proxies: OutboundProxy[];
  groups: IndexerGroup[];
  token: string;
  onClose: () => void;
  onSaved: (indexer: Indexer) => void;
  onUnauthorized: () => void;
}) {
  const [values, setValues] = useState<Record<string, ConfigurationValue>>(() =>
    initialValues(indexer),
  );
  const [userAgent, setUserAgent] = useState(indexer.userAgent ?? "");
  const [connectorId, setConnectorId] = useState(
    indexer.flaresolverrInstanceId ?? "",
  );
  const [displayName, setDisplayName] = useState(indexer.displayName);
  const [enabled, setEnabled] = useState(indexer.enabled);
  const [proxyId, setProxyId] = useState(indexer.proxyId ?? "");
  const [groupIds, setGroupIds] = useState<Set<string>>(
    () => new Set(indexer.groupIds),
  );
  const [tags, setTags] = useState(indexer.tags.join(", "));
  const [secretInputs, setSecretInputs] = useState<Record<string, string>>({});
  const [clearSecrets, setClearSecrets] = useState<Set<string>>(
    () => new Set(),
  );
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

  function toggleGroup(id: string, included: boolean) {
    setGroupIds((current) => {
      const next = new Set(current);
      if (included) next.add(id);
      else next.delete(id);
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
      const [configuration, organization] = await Promise.all([
        saveIndexerConfiguration(token, indexer.id, {
          userAgent: userAgent.trim() || null,
          flaresolverrInstanceId: connectorId || null,
          values,
          secrets,
        }),
        saveIndexerOrganization(token, indexer.id, {
          displayName: displayName.trim(),
          enabled,
          proxyId: proxyId || null,
          groupIds: Array.from(groupIds).sort(),
          tags: tags
            .split(",")
            .map((tag) => tag.trim())
            .filter(Boolean),
        }),
      ]);
      onSaved({
        ...configuration,
        displayName: organization.displayName,
        enabled: organization.enabled,
        proxyId: organization.proxyId,
        groupIds: organization.groupIds,
        tags: organization.tags,
      });
      setSecretInputs({});
      setClearSecrets(new Set());
      setStatus("Configuration saved. Secret values remain redacted.");
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) {
        onUnauthorized();
        return;
      }
      setStatus(
        error instanceof Error ? error.message : "configuration save failed",
      );
    } finally {
      setSaving(false);
    }
  }

  return (
    <form className="indexer-card" onSubmit={submit}>
      <header>
        <div>
          <p className="eyebrow">{t("Indexer configuration")}</p>
          <h3>{displayName}</h3>
          <p>{indexer.definitionId}</p>
        </div>
        <div className="editor-header-actions">
          <div className="card-badges">
            <span className={`privacy-pill ${indexer.privacy}`}>
              {indexer.privacy}
            </span>
            <span
              className={
                indexer.catalogAvailable && enabled
                  ? "status-pill healthy"
                  : "status-pill"
              }
            >
              {!indexer.catalogAvailable
                ? "unavailable (retired from catalog)"
                : enabled
                  ? "enabled"
                  : "disabled"}
            </span>
          </div>
          <button
            aria-label={t("Close indexer configuration")}
            className="icon-button"
            onClick={onClose}
            type="button"
          >
            ×
          </button>
        </div>
      </header>
      <IndexerCapabilitiesPanel indexer={indexer} />
      {indexer.configurationSchema.flaresolverrRecommended ? (
        <p className="recommendation">{t("This definition recommends FlareSolverr challenge handling.")}{" "}</p>
      ) : null}
      <div className="field-grid">
        <label className="wide-field">{t("Display name")}{" "}<input
            onChange={(event) => setDisplayName(event.target.value)}
            required
            value={displayName}
          />
        </label>
        <label className="checkbox-field wide-field">
          <input
            checked={enabled}
            onChange={(event) => setEnabled(event.target.checked)}
            type="checkbox"
          />{t("Enabled for tests and search")}{" "}</label>
        <label className="wide-field">{t("Tags (comma separated)")}{" "}<input
            onChange={(event) => setTags(event.target.value)}
            placeholder="anime, private"
            value={tags}
          />
        </label>
        <fieldset className="group-membership wide-field">
          <legend>{t("Groups")}</legend>
          <div>
            {groups.length === 0 ? (
              <small>{t("No groups configured.")}</small>
            ) : (
              groups.map((group) => (
                <label className="checkbox-field" key={group.id}>
                  <input
                    checked={groupIds.has(group.id)}
                    onChange={(event) =>
                      toggleGroup(group.id, event.target.checked)
                    }
                    type="checkbox"
                  />
                  {group.name}
                </label>
              ))
            )}
          </div>
        </fieldset>
        <label className="wide-field">{t("Outbound proxy")}{" "}<select
            onChange={(event) => setProxyId(event.target.value)}
            value={proxyId}
          >
            <option value="">{t("Inherit group or global route")}</option>
            {proxies
              .filter((proxy) => proxy.enabled)
              .map((proxy) => (
                <option key={proxy.id} value={proxy.id}>
                  {proxy.name} ({proxy.kind})
                </option>
              ))}
          </select>
        </label>
        <label className="wide-field">{t("User-Agent override")}{" "}<input
            onChange={(event) => setUserAgent(event.target.value)}
            placeholder={t("Inherit the IndexLoom default")}
            value={userAgent}
          />
        </label>
        <label className="wide-field">{t("Browser challenge route")}{" "}<select
            onChange={(event) => setConnectorId(event.target.value)}
            value={connectorId}
          >
            <option value="">{t("No FlareSolverr")}</option>
            {connectors
              .filter((connector) => connector.enabled)
              .map((connector) => (
                <option key={connector.id} value={connector.id}>
                  {connector.name}
                </option>
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
              onValue={(name, value) =>
                setSecretInputs((current) => ({ ...current, [name]: value }))
              }
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
          {saving ? "Saving changes…" : "Save changes"}
        </button>
        {status ? (
          <p className="form-status" role="status">
            {status}
          </p>
        ) : null}
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
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
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
          <small
            className={configured ? "secret-state configured" : "secret-state"}
          >
            {configured ? "configured" : "not configured"}
          </small>
        </span>
        <input
          autoComplete="new-password"
          disabled={clear}
          onChange={(event) => onValue(field.name, event.target.value)}
          placeholder={
            configured
              ? "Leave blank to keep current value"
              : "Enter secret value"
          }
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
          />{t("Clear saved value")}{" "}</label>
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

/** Only upstream-supplied categories/modes are shown; local tags stay separate. */
function IndexerCapabilitiesPanel({ indexer }: { indexer: Indexer }) {
  const caps = indexer.capabilities;
  return (
    <details className="capability-disclosure">
      <summary>
        <span>{t("Definition details")}</span>
        <span className="muted">
          {caps.source === "jackett" ? "Jackett" : "Prowlarr"} · {caps.language}
          {" · "}{caps.categoryMappings.length} mappings
        </span>
      </summary>
      <div className="capability-details">
        {caps.description ? <p>{caps.description}</p> : null}
        <dl className="capability-facts">
          <div><dt>{t("Source")}</dt><dd>{caps.source}</dd></div>
          <div><dt>{t("Language")}</dt><dd>{caps.language}</dd></div>
          <div><dt>{t("Search paths")}</dt><dd>{caps.searchPaths}</dd></div>
          <div><dt>{t("Alternative URLs")}</dt><dd>{caps.alternateUrls}</dd></div>
          <div><dt>{t("Authentication")}</dt><dd>{caps.authentication ? "Defined" : "None declared"}</dd></div>
          <div><dt>{t("CAPTCHA")}</dt><dd>{caps.captcha ? "Declared" : "Not declared"}</dd></div>
          <div><dt>{t("Download handler")}</dt><dd>{caps.downloadHandler ? "Declared" : "None declared"}</dd></div>
          <div><dt>{t("Genre query")}</dt><dd>{caps.genreSearch ? "Available in definition" : "Not declared"}</dd></div>
        </dl>
        <h4>{t("Search modes")}</h4>
        {caps.searchModes.length ? (
          <div className="capability-chip-list">
            {caps.searchModes.map((mode) => (
              <span className="capability-chip" key={mode.name} title={mode.parameters.join(", ")}>
                {mode.name} <small>{mode.parameters.join(" · ")}</small>
              </span>
            ))}
          </div>
        ) : <p className="muted">{t("No search modes declared.")}</p>}
        <h4>Categories ({caps.categoryMappings.length})</h4>
        {caps.categoryMappings.length ? (
          <div className="capability-table">
            <table>
              <thead><tr><th>{t("Tracker ID")}</th><th>{t("Category")}</th><th>{t("Description")}</th></tr></thead>
              <tbody>{caps.categoryMappings.map((category, index) => (
                <tr key={category.id + ":" + index}>
                  <td><code>{category.id}</code></td>
                  <td>{category.category}{category.default ? " (default)" : ""}</td>
                  <td>{category.description ?? "—"}</td>
                </tr>
              ))}</tbody>
            </table>
          </div>
        ) : <p className="muted">{t("No explicit category mappings; some definitions use category maps instead.")}</p>}
        <p className="muted">{t("These are declared capabilities, not a guarantee that every feature is currently supported by IndexLoom's Cardigann executor.")}</p>
      </div>
    </details>
  );
}

