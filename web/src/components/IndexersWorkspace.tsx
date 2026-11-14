import { useSystemRevision } from "../lib/events";
import { translate, categoryLabel, languageLabel, searchModeLabel, formatNumber } from "../lib/i18n";
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
} from "../lib/api";
import { AdminTokenGate, useAdminSession } from "./AdminSession";

import { BulkIndexerPanel } from "./indexers/BulkIndexerPanel";
import { ProxyPanel, GroupPanel } from "./indexers/RoutingPanels";
import { IndexerTestPanel } from "./indexers/IndexerTestPanel";
import { IndexerConfigurationPanel } from "./indexers/IndexerConfigurationPanel";
import { FlareSolverrPanel } from "./indexers/FlareSolverrPanel";
import { UsenetPanel } from "./indexers/UsenetPanel";

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
  | "bulk"
  | "catalog"
  | "tests"
  | "routing"
  | "groups"
  | "flaresolverr"
  | "usenet";

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
  { id: "bulk", label: "Bulk management", description: "Edit selected indexers" },
  { id: "tests", label: "Tests", description: "Check selected indexers" },
  {
    id: "routing",
    label: "Routing",
    description: "Global and scoped proxies",
  },
  { id: "groups", label: "Groups", description: "Organize indexers" },
  { id:"usenet",label:"Usenet",description:"Newznab NZB indexers" },
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
  const revision = useSystemRevision(["indexers", "tests", "settings", "automation"]);
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
  }, [onUnauthorized, token, revision]);

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
            <strong>{t(item.label)}</strong>
            <span>{t(item.description)}</span>
          </button>
        ))}
      </nav>

      <div className="management-panel" data-ui-section="panel">
        {managementView === "bulk" ? <BulkIndexerPanel token={token} indexers={state.indexers} groups={state.groups} onChanged={async () => { const indexers = await fetchIndexers(token, new AbortController().signal); setState(current => current.kind === "ready" ? { ...current, indexers } : current); }} /> : null}
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

        {managementView === "usenet" ? (
          <UsenetPanel token={token} onUnauthorized={onUnauthorized}
            onChanged={() => void fetchIndexers(token,new AbortController().signal).then(indexers=>
              setState(current => current.kind==="ready" ? {...current,indexers} : current))}/>
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
      <strong>{formatNumber(value)}</strong>
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

