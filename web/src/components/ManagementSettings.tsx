import { translate } from "../lib/i18n";
const t = (message: string, variables?: Record<string, string | number>) => translate(message, variables, "settings");
import { managementRequest, saveDownload } from "../lib/management-api";
import { useEffect, useState } from "react";
import { AdminTokenGate, useAdminSession } from "./AdminSession";

type Log = { id: number; timestamp: string; severity: string; component: string; message: string };
type Backup = { id: string; bytes: number };
export function ManagementSettings({ section }: { section: "logs" | "backups" }) {
  const { token, acceptToken } = useAdminSession();
  if (!token) return <AdminTokenGate onSubmit={acceptToken} />;
  return section === "logs" ? <Logs key={token} token={token} /> : <Backups key={token} token={token} />;
}
function Logs({ token }: { token: string }) {
  const [logs, setLogs] = useState<Log[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [severity, setSeverity] = useState(""); const [component, setComponent] = useState("");
  const [error, setError] = useState(""); const [busy, setBusy] = useState(false);
  async function load(older = false) {
    setBusy(true); setError("");
    try {
      const params = new URLSearchParams({ limit: "100" });
      if (severity) params.set("severity", severity); if (component) params.set("component", component);
      const last = logs.at(-1);
      if (older && last) params.set("before", String(last.id));
      const rows = await (await managementRequest(token, `diagnostics/logs?${params}`)).json() as Log[];
      setLogs(current => older ? [...current, ...rows] : rows);
      setHasMore(rows.length === 100);
    } catch (e) { setError(e instanceof Error ? e.message : "Cannot load logs"); } finally { setBusy(false); }
  }
  useEffect(() => { void load(); }, [token]);
  return <section className="appearance-content" role="tabpanel"><h2>{t("Logs / Diagnostics")}</h2>
    <p>{t("Structured runtime diagnostics, separate from activity history. The latest 2,000 events are retained. Messages exclude request data, credentials and error details.")}</p>
    <label>{t("Severity")}<select disabled={busy} value={severity} onChange={e => { setSeverity(e.target.value); setLogs([]); }}><option value="">{t("All")}</option>{["info", "warn", "error"].map(s => <option key={s}>{s}</option>)}</select></label>
    <label>{t("Component")}<select disabled={busy} value={component} onChange={e => { setComponent(e.target.value); setLogs([]); }}><option value="">{t("All")}</option>{["server", "catalog", "backups", "scheduler"].map(s => <option key={s}>{s}</option>)}</select></label>
    <button disabled={busy} onClick={() => void load()}>{t("Apply filters / Refresh")}</button>
    <button onClick={() => void saveDownload(new Response(JSON.stringify(logs, null, 2), { headers: { "Content-Type": "application/json" } }), "indexloom-diagnostics.json")}>{t("Download displayed logs")}</button>
    <p role="alert">{error}</p>
    <p role="status">{busy ? t("Loading diagnostic events…") : t("{count} events displayed", { count: logs.length })}</p>
    <ul>{logs.map(log => <li key={log.id}><time>{log.timestamp}</time> <strong>{log.severity}</strong> [{log.component}] {log.message}</li>)}</ul>
    <button disabled={busy || !hasMore || logs.length === 0} onClick={() => void load(true)}>{t("Load older events")}</button>
  </section>;
}
function Backups({ token }: { token: string }) {
  const [backups, setBackups] = useState<Backup[]>([]); const [queued, setQueued] = useState(false);
  const [message, setMessage] = useState(""); const [error, setError] = useState(""); const [busy, setBusy] = useState(false);
  async function refresh() {
    const data = await (await managementRequest(token, "backups")).json() as { backups: Backup[]; restoreQueued: boolean };
    setBackups(data.backups); setQueued(data.restoreQueued);
  }
  async function run(action: () => Promise<void>, reload = true) {
    setBusy(true); setError(""); setMessage("");
    try { await action(); if (reload) await refresh(); } catch (e) { setError(e instanceof Error ? e.message : "Backup request failed"); } finally { setBusy(false); }
  }
  useEffect(() => { void run(refresh, false); }, [token]);
  return <section className="appearance-content" role="tabpanel"><h2>{t("Backups")}</h2>
    <p>{t("Owner access required. Up to 10 verified SQLite snapshots, each at most 256 MiB. Backups contain encrypted credentials; retain your master key separately.")}</p>
    <p>{t("Restore is queued for the next server restart. Restart must be performed by the operator. Writes made after queueing will be replaced; an offline rollback copy is retained. Restore requires a stopped, checkpointed database without WAL/SHM files.")}</p>
    <p role="status">{queued ? "Restore queued — awaiting server restart. It has not been applied." : message}</p><p role="alert">{error}</p>
    <button disabled={busy} onClick={() => void run(async () => { await managementRequest(token, "backups", { method: "POST" }); setMessage("Backup created and verified."); })}>{t("Create backup")}</button>
    <button disabled={busy} onClick={() => void run(refresh, false)}>{t("Refresh")}</button>
    <label>{t("Upload SQLite backup")}<input type="file" accept=".db,.sqlite,.sqlite3" disabled={busy} onChange={e => {
      const file = e.target.files?.[0]; e.target.value = ""; if (!file) return;
      void run(async () => { if (file.size > 256 * 1024 * 1024) throw new Error("Backup exceeds 256 MiB"); await managementRequest(token, "backups/upload", { method: "POST", body: file, headers: { "Content-Type": "application/octet-stream" } }); setMessage("Uploaded backup verified. No restore has been queued."); });
    }} /></label>
    <ul>{backups.map(backup => <li key={backup.id}><code>{backup.id}</code> ({backup.bytes.toLocaleString()} bytes)
      <button disabled={busy} onClick={() => void run(async () => saveDownload(await managementRequest(token, `backups/${backup.id}`), `indexloom-${backup.id}.db`))}>{t("Download")}</button>
      <button disabled={busy || queued} onClick={() => {
        if (window.prompt("This replaces the database on restart. Type RESTORE ON RESTART to queue this backup:") !== "RESTORE ON RESTART") return;
        void run(async () => { await managementRequest(token, `backups/${backup.id}/restore`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ confirmation: "RESTORE ON RESTART" }) }); setMessage("Restore queued; restart required."); });
      }}>{t("Queue restore")}</button>
      <button disabled={busy} onClick={() => { if (window.confirm(`Delete backup ${backup.id}?`)) void run(async () => { await managementRequest(token, `backups/${backup.id}`, { method: "DELETE" }); }); }}>{t("Delete")}</button>
    </li>)}</ul>
  </section>;
}
