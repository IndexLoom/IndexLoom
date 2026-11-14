import { useSystemRevision } from "../lib/events";
import { useEffect, useState } from "react";
import { AdminTokenGate, useAdminSession } from "./AdminSession";
import { automationRequest } from "./IntegrationSettings";
import { translate } from "../lib/i18n";
const t = (message: string) => translate(message, undefined, "settings");
type Job = { id: string; kind: string; target: string | null; intervalSeconds: number; enabled: boolean; nextRun: number; lastRun: number | null; lastOutcome: string | null };
type Quota = { indexerId: string; hourlyLimit: number; dailyLimit: number; hourUsed: number; dayUsed: number; retryAfterSeconds: number; nextAllowedAt: number | null };
type Entry = { id: number; indexerId: string; title: string; createdAt: number };
const blank = { id: "", kind: "app-sync", target: "", intervalSeconds: 3600, enabled: true };
export function AutomationSettings() {
  const systemRevision = useSystemRevision(["automation", "settings"]);
  const { token, acceptToken } = useAdminSession();
  const [jobs, setJobs] = useState<Job[]>([]); const [quotas, setQuotas] = useState<Quota[]>([]); const [history, setHistory] = useState<Entry[]>([]);
  const [job, setJob] = useState(blank); const [quota, setQuota] = useState({ id: "", hourlyLimit: 0, dailyLimit: 0 });
  const [busy, setBusy] = useState(false); const [error, setError] = useState(""); const [message, setMessage] = useState("");
  function request<T>(path: string, method?: string, body?: unknown): Promise<T> { return automationRequest<T>(token ?? "", "automation/" + path, method, body); }
  async function load() {
    const [j, q, h] = await Promise.all([request<Job[]>("jobs"), request<Quota[]>("quotas"), request<Entry[]>("rss/history")]);
    setJobs(j); setQuotas(q); setHistory(h);
  }
  useEffect(() => { if (token) void load().catch(() => setError(t("Could not load automation settings. Refresh to retry."))); }, [token, systemRevision]);
  async function run(action: () => Promise<void>) {
    setBusy(true); setError(""); setMessage("");
    try { await action(); await load(); setMessage(t("Automation settings updated.")); } catch (e) { setError(e instanceof Error ? e.message : t("Request failed. Check the settings and try again.")); } finally { setBusy(false); }
  }
  if (!token) return <AdminTokenGate onSubmit={acceptToken} />;
  return <div className="appearance-content automation-settings">
    <div className="setting-heading"><h2>{t("Scheduled tasks")}</h2><p>{t("Tasks persist across restarts. RSS starts with a silent baseline and records later new entries in history.")}</p></div>
    <button disabled={busy} type="button" onClick={() => { void run(async () => {}); }}>{t("Refresh status")}</button>
    {jobs.length === 0 ? <p>{t("No scheduled tasks. Add an application sync or RSS task below.")}</p> : <ul>{jobs.map((j) => <li key={j.id}>
      <strong>{j.id}</strong> · {j.kind} · {j.target ?? ""} · {j.enabled ? t("Enabled") : t("Disabled")} · {j.lastOutcome ?? t("Not run yet")} · {t("Next run")}: {new Date(j.nextRun * 1000).toLocaleString()}{" "}
      <button disabled={busy} type="button" onClick={() => setJob({ id: j.id, kind: j.kind, target: j.target ?? "", intervalSeconds: j.intervalSeconds, enabled: j.enabled })}>{t("Edit task")}</button>{" "}
      <button disabled={busy || !j.enabled} type="button" onClick={() => { void run(async () => { await request("jobs/" + encodeURIComponent(j.id) + "/run", "POST"); }); }}>{t("Run now")}</button>{" "}
      <button disabled={busy} type="button" onClick={() => { void run(async () => { await request("jobs/" + encodeURIComponent(j.id), "DELETE"); }); }}>{t("Delete task")}</button>
    </li>)}</ul>}
    <form onSubmit={(e) => { e.preventDefault(); void run(async () => { await request("jobs/" + encodeURIComponent(job.id), "PUT", { kind: job.kind, target: job.kind === "rss" || job.kind === "app-sync" ? job.target : null, intervalSeconds: job.intervalSeconds, enabled: job.enabled }); setJob(blank); }); }}>
      <fieldset disabled={busy}><legend>{t("Schedule task")}</legend><div className="appearance-controls">
        <label>{t("Task ID")}<input required value={job.id} onChange={(e) => setJob({ ...job, id: e.target.value })} /></label>
        <label>{t("Task type")}<select value={job.kind} onChange={(e) => setJob({ ...job, kind: e.target.value })}><option value="app-sync">{t("Application sync")}</option><option value="rss">{t("RSS scan")}</option><option value="maintenance">{t("Maintenance")}</option><option value="catalog">{t("Catalog sync")}</option></select></label>
        {job.kind === "rss" || job.kind === "app-sync" ? <label>{t("Application or indexer ID")}<input required value={job.target} onChange={(e) => setJob({ ...job, target: e.target.value })} /></label> : null}
        <label>{t("Interval in seconds")}<input required type="number" min={60} max={2592000} value={job.intervalSeconds} onChange={(e) => setJob({ ...job, intervalSeconds: Number(e.target.value) })} /></label>
        <label><input type="checkbox" checked={job.enabled} onChange={(e) => setJob({ ...job, enabled: e.target.checked })} />{t("Enable task")}</label>
      </div><button type="submit">{t("Save task")}</button></fieldset>
    </form>
    <div className="setting-heading"><h2>{t("Search quotas")}</h2><p>{t("Budgets apply to interactive searches and RSS requests. Zero means unlimited. Windows reset at the UTC hour and day boundaries; cache hits use no budget.")}</p></div>
    <ul>{quotas.map((q) => <li key={q.indexerId}><strong>{q.indexerId}</strong> · {t("Hourly")}: {q.hourUsed}/{q.hourlyLimit || "∞"} · {t("Daily")}: {q.dayUsed}/{q.dailyLimit || "∞"}{" "}{q.retryAfterSeconds > 0 ? <span> · {t("Next search in")}: {Math.ceil(q.retryAfterSeconds / 60)} {t("minutes")}</span> : null}{" "}<button disabled={busy} type="button" onClick={() => setQuota({ id: q.indexerId, hourlyLimit: q.hourlyLimit, dailyLimit: q.dailyLimit })}>{t("Edit quota")}</button></li>)}</ul>
    <form onSubmit={(e) => { e.preventDefault(); void run(async () => { await request("quotas/" + encodeURIComponent(quota.id), "PUT", { hourlyLimit: quota.hourlyLimit, dailyLimit: quota.dailyLimit }); }); }}>
      <fieldset disabled={busy}><legend>{t("Set indexer quota")}</legend><div className="appearance-controls">
        <label>{t("Indexer ID")}<input required value={quota.id} onChange={(e) => setQuota({ ...quota, id: e.target.value })} /></label>
        <label>{t("Hourly limit")}<input required type="number" min={0} value={quota.hourlyLimit} onChange={(e) => setQuota({ ...quota, hourlyLimit: Number(e.target.value) })} /></label>
        <label>{t("Daily limit")}<input required type="number" min={0} value={quota.dailyLimit} onChange={(e) => setQuota({ ...quota, dailyLimit: Number(e.target.value) })} /></label>
      </div><button type="submit">{t("Save quota")}</button></fieldset>
    </form>
    <div className="setting-heading"><h2>{t("RSS history")}</h2></div>
    {history.length === 0 ? <p>{t("No new entries recorded. Schedule an RSS scan; the first successful scan establishes the baseline.")}</p> : <ul>{history.map((h) => <li key={h.id}>{h.indexerId}: {h.title} · {new Date(h.createdAt * 1000).toLocaleString()}</li>)}</ul>}
    <p role="status">{busy ? t("Working…") : message}</p>{error ? <p role="alert" className="inline-error">{error}</p> : null}
  </div>;
}
