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

import { EMPTY_PROXY, EMPTY_GROUP, type ProxyDraft, type GroupDraft } from "./shared";
export function ProxyPanel({
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
        t("Delete this outbound proxy? Global, group and indexer assignments will fall back automatically."),
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
              {t(saving ? "Saving…" : editing ? "Update proxy" : "Add proxy")}
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

export function GroupPanel({
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
        t("Delete this group? Indexers stay installed and only membership is removed."),
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
              {t(status)}
            </p>
          ) : null}
        </form>
      </div>
    </section>
  );
}

