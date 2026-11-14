import { useEffect, useRef, useState } from "react";
import { useAdminSession } from "./AdminSession";
import { eventQuery, monitoringRequest, normalizeEvents, normalizeHealth, normalizeIndexers, type EventPage, type Health, type IndexerStats } from "../lib/monitoring";
import { metric, Status, t, timestamp } from "./monitoring/shared";
import { AuditTable } from "./monitoring/AuditTable";
import "./monitoring/monitoring.css";

type Resource<T> = { data: T | null; error: string; loading: boolean; updated: string };
function useResource<T>(path: string, token: string, normalize: (value: unknown) => T, revision: number) {
  const [state, setState] = useState<Resource<T>>({ data: null, error: "", loading: true, updated: "" });
  useEffect(() => {
    const controller = new AbortController();
    setState({ data: null, error: "", loading: true, updated: "" });
    void monitoringRequest(path, token, controller.signal).then(normalize).then((data) => {
      if (!controller.signal.aborted) setState({ data, loading: false, error: "", updated: new Date().toISOString() });
    }).catch((e: unknown) => { if (!controller.signal.aborted) setState({ data: null, loading: false, error: e instanceof Error ? e.message : "Monitoring unavailable", updated: "" }); });
    return () => controller.abort();
  }, [path, token, normalize, revision]);
  return state;
}
const eventNormalizer = (value: unknown) => normalizeEvents(value, 25);
const initialFilters = { indexerId: "", kind: "", outcome: "", before: "", after: "" };
const healthLabels: Record<string, string> = { database: "Database", indexers: "Indexers", proxies: "Proxies", connectedApps: "Connected apps", mail: "Mail", rssScheduler: "RSS scheduler", systemTime: "System time" };
const healthAdvice: Record<Health["status"], string> = { healthy: "No action needed", degraded: "Check configuration and connectivity", unconfigured: "Configure this dependency if needed", unknown: "No health result reported; refresh to retry" };
export function MonitoringWorkspace() {
  const { token } = useAdminSession();
  const [revision, setRevision] = useState(0), [autoRefresh, setAutoRefresh] = useState(false);
  const [filters, setFilters] = useState(initialFilters);
  const [cursors, setCursors] = useState<Array<number | null>>([null]);
  const pageIndex = cursors.length - 1;
  const [search, setSearch] = useState(""), [status, setStatus] = useState("");
  const [sort, setSort] = useState<{ key: keyof IndexerStats; descending: boolean }>({ key: "name", descending: false });
  const [authorizedToken, setAuthorizedToken] = useState<string | null>(null);
  const [resetting, setResetting] = useState(""), [resetMessage, setResetMessage] = useState("");
  const resetController = useRef<AbortController | null>(null);
  const indexers = useResource("/api/v1/monitoring/indexers", token, normalizeIndexers, revision);
  const health = useResource("/api/v1/monitoring/health", token, normalizeHealth, revision);
  const invalidDates = Boolean(filters.before && filters.after && filters.after > filters.before);
  const path = eventQuery(invalidDates ? { ...filters, after: "" } : filters, cursors[pageIndex] ?? null, 25);
  const events = useResource<EventPage>(path, token, eventNormalizer, revision);
  useEffect(() => {
    const controller = new AbortController();
    setAuthorizedToken(null);
    setResetting(""); setResetMessage("");
    void monitoringRequest("/api/v1/auth/me", token, controller.signal).then((value) => {
      const session = value as { user?: { role?: string } } | null;
      if (!controller.signal.aborted && ["owner", "admin"].includes(session?.user?.role ?? "")) setAuthorizedToken(token);
    }).catch(() => {});
    return () => { controller.abort(); resetController.current?.abort(); };
  }, [token]);
  useEffect(() => {
    if (!autoRefresh) return;
    const interval = window.setInterval(() => { if (!document.hidden && !resetController.current) setRevision((v) => v + 1); }, 30000);
    return () => window.clearInterval(interval);
  }, [autoRefresh]);
  async function reset(item: IndexerStats) {
    if (authorizedToken !== token || resetting || !window.confirm(t("Reset circuit breaker?") + " " + item.name + ". " + t("This allows an immediate retry. It does not enable a disabled indexer."))) return;
    const controller = new AbortController(); resetController.current = controller;
    setResetting(item.id); setResetMessage("");
    try {
      await monitoringRequest("/api/v1/monitoring/indexers/" + encodeURIComponent(item.id) + "/reset", token, controller.signal, "POST");
      if (!controller.signal.aborted) { setResetMessage(t("Circuit breaker reset")); setRevision((v) => v + 1); }
    } catch (e) { if (!controller.signal.aborted) setResetMessage(e instanceof Error ? e.message : t("Reset failed")); }
    finally { if (!controller.signal.aborted) setResetting(""); if (resetController.current === controller) resetController.current = null; }
  }
  const items = indexers.data ?? [];
  const visible = items.filter((item) => item.name.toLocaleLowerCase().includes(search.toLocaleLowerCase()) && (!status || item.status === status)).sort((a, b) => {
    const av = a[sort.key], bv = b[sort.key];
    if (av === null) return bv === null ? 0 : 1;
    if (bv === null) return -1;
    return (typeof av === "number" && typeof bv === "number" ? av - bv : String(av).localeCompare(String(bv))) * (sort.descending ? -1 : 1);
  });
  const columns: [keyof IndexerStats, string][] = [["name", "Indexer"], ["status", "Status"], ["requests", "Requests"], ["failures", "Failures"], ["authErrors", "Auth errors"], ["rssOperations", "RSS operations"], ["downloads", "Downloads"], ["meanResponseMs", "Mean response (ms)"], ["lastEvent", "Last event"]];
  return <section className="monitoring-view">
    <header className="page-heading"><div><h1>{t("Monitoring")}</h1><p className="page-description">{t("Server statistics, audit activity and dependency health.")}</p></div><button className="quiet-button" onClick={() => setRevision((v) => v + 1)} type="button">{t("Refresh")}</button></header>
    <label className="monitoring-auto"><input type="checkbox" checked={autoRefresh} onChange={(e) => setAutoRefresh(e.target.checked)} />{t("Refresh every 30 seconds while visible")}</label>
    <section className="monitoring-panel" aria-labelledby="monitoring-indexers"><h2 id="monitoring-indexers">{t("Indexer statistics")}</h2>
      <LoadState state={indexers} />
      {indexers.data ? <><div className="monitoring-totals">{(["requests", "failures", "authErrors", "rssOperations", "downloads"] as const).map((key) => <span key={key}>{t(columns.find(([k]) => k === key)![1])}: <strong>{metric(items.some((i) => i[key] === null) ? null : items.reduce((sum, i) => sum + (i[key] ?? 0), 0))}</strong></span>)}</div>
      <div className="monitoring-filters"><label>{t("Find indexer")}<input type="search" value={search} onChange={(e) => setSearch(e.target.value)} /></label><label>{t("Status")}<select value={status} onChange={(e) => setStatus(e.target.value)}><option value="">{t("All")}</option>{["enabled", "disabled", "circuit-open", "half-open", "unknown"].map((s) => <option key={s} value={s}>{t(s)}</option>)}</select></label><button type="button" className="quiet-button" onClick={() => { setSearch(""); setStatus(""); }}>{t("Clear filters")}</button></div>
      <div className="monitoring-scroll" tabIndex={0} role="region" aria-label={t("Indexer statistics")}><table><caption>{t("Indexer statistics")} · {visible.length}</caption><thead><tr>{columns.map(([key, label]) => <th scope="col" key={key} aria-sort={sort.key === key ? sort.descending ? "descending" : "ascending" : undefined}><button type="button" onClick={() => setSort({ key, descending: sort.key === key && !sort.descending })}>{t(label)}{sort.key === key ? sort.descending ? " ↓" : " ↑" : ""}</button></th>)}<th scope="col">{t("Circuit breaker")}</th></tr></thead><tbody>{visible.map((item) => <tr key={item.id}>{columns.map(([key]) => <td key={key}>{key === "name" ? <strong>{item.name}</strong> : key === "status" ? <Status value={item.status} /> : key === "lastEvent" ? timestamp(item.lastEvent) : metric(item[key] as number | null)}</td>)}<td><span>{t("Failure streak")}: {metric(item.failureStreak)} · {t("Backoff (s)")}: {metric(item.backoffSeconds)}</span>{authorizedToken === token && ["circuit-open", "half-open"].includes(item.status) ? <button type="button" className="quiet-button" disabled={Boolean(resetting)} onClick={() => void reset(item)} aria-label={t("Reset circuit breaker") + ": " + item.name}>{t(resetting === item.id ? "Resetting…" : "Reset circuit breaker")}</button> : null}</td></tr>)}</tbody></table></div>{visible.length === 0 ? <p>{t("No indexers match. Clear filters or configure an indexer.")}</p> : null}</> : null}
      <p className="muted">{t("Circuit-open pauses requests temporarily with automatic cooldown. Half-open probes recovery. Disabled indexers stay disabled until enabled in configuration.")}</p><p role="status">{resetMessage}</p>
    </section>
    <section className="monitoring-panel" aria-labelledby="monitoring-health"><h2 id="monitoring-health">{t("Dependency health")}</h2><LoadState state={health} />{health.data ? <ul className="monitoring-health">{health.data.map((item) => <li key={item.id}><strong>{t(healthLabels[item.id] ?? item.id)}</strong><Status value={item.status} /><span className="muted">{t(healthAdvice[item.status])}</span></li>)}</ul> : null}</section>
    <section className="monitoring-panel" aria-labelledby="monitoring-audit"><h2 id="monitoring-audit">{t("Audit activity")}</h2><p className="muted">{t("Server audit records. Search queries and raw request details are not shown.")}</p><AuditTable filters={filters} onFilter={(key, value) => { setFilters((f) => ({ ...f, [key]: value })); setCursors([null]); }} indexers={items} page={events.data} loading={events.loading} invalidDates={invalidDates} pageIndex={pageIndex} onPrevious={() => setCursors((xs) => xs.slice(0, -1))}
      onNext={() => { if (events.data?.nextBefore !== null && events.data?.nextBefore !== undefined) setCursors((xs) => [...xs, events.data!.nextBefore]); }} /><LoadState state={events} /></section>
  </section>;
}
function LoadState({ state }: { state: Resource<unknown> }) { return <><p role="status" className="muted">{state.loading ? t("Loading monitoring…") : state.updated ? t("Updated") + " · " + timestamp(state.updated) : ""}</p>{state.error ? <p role="alert" className="inline-error">{t(state.error)}. {t("Use Refresh to retry. Check server availability and account permissions.")}</p> : null}</>; }
