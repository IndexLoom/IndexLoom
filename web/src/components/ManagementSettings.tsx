import { formatDateTime, formatNumber, translate } from "../lib/i18n";
const t = (message: string, variables?: Record<string, string | number>) => translate(message, variables, "settings");
import { managementRequest, saveDownload } from "../lib/management-api";
import { useEffect, useRef, useState } from "react";
import { AdminTokenGate, useAdminSession } from "./AdminSession";

type Log = { id: number; timestamp: string; severity: string; component: string; message: string };
const severityLabels: Record<string, string> = { info: "Information", warn: "Warning", error: "Error" };
const componentLabels: Record<string, string> = { server: "Server", catalog: "Catalog", backups: "Backups", scheduler: "Scheduler" };
export const backupSize = (bytes: number) => `${formatNumber(Math.round(bytes / 1024 / 1024 * 100) / 100)} ${t("MiB")}`;
type Backup = { id: string; bytes: number };
export function ManagementSettings({ section }: { section: "logs" | "backups" }) {
  const { token, acceptToken } = useAdminSession();
  if (!token) return <AdminTokenGate onSubmit={acceptToken} />;
  return section === "logs" ? <Logs key={token} token={token} /> : <Backups key={token} token={token} />;
}
function Logs({ token }: { token: string }) {
  const request = useRef<AbortController | null>(null);
  const [logs, setLogs] = useState<Log[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [severity, setSeverity] = useState(""); const [component, setComponent] = useState("");
  const [error, setError] = useState(""); const [busy, setBusy] = useState(false);
  async function load(older = false) {
    request.current?.abort();
    const controller = new AbortController(); request.current = controller;
    setBusy(true); setError("");
    try {
      const params = new URLSearchParams({ limit: "100" });
      if (severity) params.set("severity", severity); if (component) params.set("component", component);
      const last = logs.at(-1);
      if (older && last) params.set("before", String(last.id));
      const rows = await (await managementRequest(token, `diagnostics/logs?${params}`, { signal: controller.signal })).json() as Log[];
      if (controller.signal.aborted) return;
      setLogs(current => older ? [...current, ...rows] : rows);
      setHasMore(rows.length === 100);
    } catch (e) { if (!controller.signal.aborted) setError(e instanceof Error ? e.message : "Cannot load logs"); } finally { if (!controller.signal.aborted) setBusy(false); }
  }
  useEffect(() => { void load(); return () => request.current?.abort(); }, [token, severity, component]);
  return <section className="appearance-content operations-panel"><h2>{t("Logs / Diagnostics")}</h2>
    <p>{t("Structured runtime diagnostics, separate from activity history. The latest 2,000 events are retained. Messages exclude request data, credentials and error details.")}</p>
    <div className="operations-toolbar"><label>{t("Severity")}<select disabled={busy} value={severity} onChange={e => { setSeverity(e.target.value); }}><option value="">{t("All")}</option>{["info", "warn", "error"].map(s => <option key={s} value={s}>{t(severityLabels[s] ?? s)}</option>)}</select></label>
    <label>{t("Component")}<select disabled={busy} value={component} onChange={e => { setComponent(e.target.value); }}><option value="">{t("All")}</option>{["server", "catalog", "backups", "scheduler"].map(s => <option key={s} value={s}>{t(componentLabels[s] ?? s)}</option>)}</select></label>
    <button disabled={busy} onClick={() => void load()}>{t("Apply filters / Refresh")}</button>
    <button disabled={busy || logs.length === 0} onClick={() => void saveDownload(new Response(JSON.stringify(logs, null, 2), { headers: { "Content-Type": "application/json" } }), "indexloom-diagnostics.json")}>{t("Download displayed logs")}</button>
    <button disabled={busy || logs.length === 0} onClick={() => { void (navigator.clipboard?.writeText(JSON.stringify(logs, null, 2)) ?? Promise.reject()).then(() => setError(""), () => setError(t("Could not copy logs. Download displayed logs instead."))); }}>{t("Copy displayed logs")}</button></div>
    {error ? <p role="alert">{t(error)}</p> : null}
    <p role="status">{busy ? t("Loading diagnostic events…") : t("{{count}} events displayed", { count: logs.length })}</p>
    <DiagnosticsTable logs={logs} />
    {!busy && logs.length === 0 ? <p className="muted">{t("No diagnostic events match. Change filters or refresh.")}</p> : null}
    <button disabled={busy || !hasMore || logs.length === 0} onClick={() => void load(true)}>{t("Load older events")}</button>
  </section>;
}
export function DiagnosticsTable({ logs }: { logs: Log[] }) {
  return <div className="operations-scroll" tabIndex={0} role="region" aria-label={t("Logs / Diagnostics")}><table className="operations-table"><thead><tr>{["Time", "Severity", "Component", "Message"].map(label => <th scope="col" key={label}>{t(label)}</th>)}</tr></thead><tbody>{logs.map(log => <tr key={log.id}><td><time dateTime={log.timestamp}>{formatDateTime(log.timestamp)}</time></td><td><span className="monitoring-status" data-status={log.severity}>{t(severityLabels[log.severity] ?? log.severity)}</span></td><td>{t(componentLabels[log.component] ?? log.component)}</td><td className="operations-message">{log.message}</td></tr>)}</tbody></table></div>;
}
export async function queueBackupRestore(token: string, id: string, confirm: (message: string) => string | null = message => window.prompt(message)) {
  if (confirm(t("Restore backup {{id}} on restart? This replaces the database, including writes made after queueing. Type RESTORE ON RESTART to confirm:", { id })) !== "RESTORE ON RESTART") return false;
  await managementRequest(token, `backups/${encodeURIComponent(id)}/restore`, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ confirmation: "RESTORE ON RESTART" }),
  });
  return true;
}
function Backups({ token }: { token: string }) {
  const [loaded, setLoaded] = useState(false);
  const [backups, setBackups] = useState<Backup[]>([]); const [queued, setQueued] = useState(false);
  const [message, setMessage] = useState(""); const [error, setError] = useState(""); const [busy, setBusy] = useState(false);
  async function refresh() {
    const data = await (await managementRequest(token, "backups")).json() as { backups: Backup[]; restoreQueued: boolean };
    setBackups(data.backups); setQueued(data.restoreQueued); setLoaded(true);
  }
  async function run(action: () => Promise<void>, reload = true) {
    setBusy(true); setError(""); setMessage("");
    try { await action(); if (reload) await refresh(); } catch (e) { setError(e instanceof Error ? e.message : "Backup request failed"); } finally { setBusy(false); }
  }
  useEffect(() => { void run(refresh, false); }, [token]);
  return <section className="appearance-content operations-panel"><h2>{t("Backups")}</h2>
    <p>{t("Owner access required. Up to 10 verified SQLite snapshots, each at most 256 MiB. Backups contain encrypted credentials; retain your master key separately.")}</p>
    <p>{t("Restore is queued for the next server restart. Restart must be performed by the operator. Writes made after queueing will be replaced; an offline rollback copy is retained. Restore requires a stopped, checkpointed database without WAL/SHM files.")}</p>
    <p role="status">{busy ? t("Processing backup request…") : queued ? t("Restore queued — awaiting server restart. It has not been applied.") : message ? t(message) : loaded ? t("No restore queued.") : ""}</p>{error ? <p role="alert">{t(error)}</p> : null}
    <div className="operations-toolbar"><button disabled={busy} onClick={() => void run(async () => { await managementRequest(token, "backups", { method: "POST" }); setMessage("Backup created and verified."); })}>{t("Create backup")}</button>
    <button disabled={busy} onClick={() => void run(refresh, false)}>{t("Refresh")}</button>
    <label>{t("Upload SQLite backup")}<input type="file" accept=".db,.sqlite,.sqlite3" disabled={busy} onChange={e => {
      const file = e.target.files?.[0]; e.target.value = ""; if (!file) return;
      void run(async () => { if (file.size > 256 * 1024 * 1024) throw new Error("Backup exceeds 256 MiB"); await managementRequest(token, "backups/upload", { method: "POST", body: file, headers: { "Content-Type": "application/octet-stream" } }); setMessage("Uploaded backup verified. No restore has been queued."); });
    }} /></label></div>
    <div className="operations-scroll" tabIndex={0} role="region" aria-label={t("Backups")}><table className="operations-table"><thead><tr>{["Backup", "Size", "Actions"].map(label => <th scope="col" key={label}>{t(label)}</th>)}</tr></thead><tbody>{backups.map(backup => <tr key={backup.id}><td><code>{backup.id}</code></td><td title={`${backup.bytes} ${t("bytes")}`}>{backupSize(backup.bytes)}</td><td><div className="operations-actions">
      <button disabled={busy} onClick={() => void run(async () => saveDownload(await managementRequest(token, `backups/${encodeURIComponent(backup.id)}`), `indexloom-${backup.id}.db`))}>{t("Download")}</button>
      <button disabled={busy || queued || !loaded} className="operations-danger" onClick={() => void run(async () => {
        if (await queueBackupRestore(token, backup.id)) setMessage("Restore queued; restart required.");
      })}>{t("Queue restore")}</button>
      <button disabled={busy || queued} onClick={() => { if (window.confirm(t("Delete backup {{id}}?", { id: backup.id }))) void run(async () => { await managementRequest(token, `backups/${encodeURIComponent(backup.id)}`, { method: "DELETE" }); }); }}>{t("Delete")}</button>
    </div></td></tr>)}</tbody></table></div>
    {loaded && backups.length === 0 ? <p className="muted">{t("No backups yet. Create a snapshot or upload a SQLite backup.")}</p> : null}
  </section>;
}
