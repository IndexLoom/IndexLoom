import { useEffect, useState } from "react";
import { AdminTokenGate, useAdminSession } from "./AdminSession";
import { translate } from "../lib/i18n";
const t = (message: string) => translate(message, undefined, "settings");
type App = { id: string; provider: string; name: string; origin: string; enabled: boolean };
type Profile = { mode: "Disabled" | "AddOnly" | "FullSync"; feedOrigin: string | null; indexerIds: string[]; groupIds: string[]; tags: string[]; categories: number[]; remoteTags: number[]; priority: number };
const blankApp: App = { id: "", provider: "sonarr", name: "", origin: "", enabled: true };
const blankProfile: Profile = { mode: "Disabled", feedOrigin: null, indexerIds: [], groupIds: [], tags: [], categories: [], remoteTags: [], priority: 25 };
const split = (text: string) => text.split(",").map((s) => s.trim()).filter(Boolean);
export async function automationRequest<T>(token: string, path: string, method = "GET", body?: unknown): Promise<T> {
  const response = await fetch("/api/v1/" + path, { method, cache: "no-store", credentials: "omit", headers: { Authorization: "Bearer " + token, "Content-Type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  if (!response.ok) {
    const error = await response.json().catch(() => null) as { message?: string; error?: { message?: string } } | null;
    throw new Error(error?.message ?? error?.error?.message ?? t("Request failed. Check the settings and try again."));
  }
  return await response.json() as T;
}
export function IntegrationSettings() {
  const { token, acceptToken } = useAdminSession();
  const [apps, setApps] = useState<App[]>([]);
  const [app, setApp] = useState<App>(blankApp);
  const [profile, setProfile] = useState<Profile>(blankProfile);
  const [apiKey, setApiKey] = useState(""); const [feedKey, setFeedKey] = useState("");
  const [busy, setBusy] = useState(false); const [message, setMessage] = useState(""); const [error, setError] = useState("");
  useEffect(() => { if (token) void automationRequest<App[]>(token, "integrations").then(setApps).catch(() => setError(t("Could not load applications. Refresh this page to retry."))); }, [token]);
  if (!token) return <AdminTokenGate onSubmit={acceptToken} />;
  function request<T>(path: string, method?: string, body?: unknown): Promise<T> { return automationRequest<T>(token, path, method, body); }
  async function run(action: () => Promise<void>) {
    setBusy(true); setError(""); setMessage("");
    try { await action(); } catch (e) { setError(e instanceof Error ? e.message : t("Could not save. Check the settings and try again.")); } finally { setBusy(false); }
  }
  const base = "integrations/" + encodeURIComponent(app.id);
  const existing = apps.some((a) => a.id === app.id);
  return <div className="appearance-content integration-settings">
    <div className="setting-heading"><h2>{t("Applications")}</h2><p>{t("Synchronize indexers with Sonarr, Radarr, Lidarr, Readarr, Whisparr, Mylar3 or LazyLibrarian.")}</p></div>
    <label>{t("Application")}<select disabled={busy} value={existing ? app.id : ""} onChange={(e) => { const value = apps.find((a) => a.id === e.target.value); void run(async () => { setApiKey(""); setFeedKey(""); const nextProfile = value ? await request<Profile>("integrations/" + encodeURIComponent(value.id) + "/profile") : blankProfile; setApp(value ?? blankApp); setProfile(nextProfile); }); }}><option value="">{t("New application")}</option>{apps.map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}</select></label>
    <form onSubmit={(e) => { e.preventDefault(); void run(async () => {
      if ((apiKey || feedKey) && !(apiKey && feedKey)) throw new Error(t("Enter both API keys to replace credentials."));
      await request(base, "PUT", { ...app, ...(apiKey && feedKey ? { secrets: { apiKey, feedApiKey: feedKey } } : {}) });
      setApiKey(""); setFeedKey(""); setApps(await request<App[]>("integrations")); setMessage(t("Application saved."));
    }); }}>
      <fieldset disabled={busy}><legend>{t("Connection")}</legend><div className="appearance-controls">
        <label>{t("ID")}<input required readOnly={existing} value={app.id} onChange={(e) => setApp({ ...app, id: e.target.value })} /></label>
        <label>{t("Name")}<input required maxLength={100} value={app.name} onChange={(e) => setApp({ ...app, name: e.target.value })} /></label>
        <label>{t("Provider")}<select disabled={existing} value={app.provider} onChange={(e) => setApp({ ...app, provider: e.target.value })}>{["sonarr", "radarr", "lidarr", "readarr", "whisparr", "mylar", "lazylibrarian"].map((p) => <option key={p}>{p}</option>)}</select></label>
        <label>{t("Application origin")}<input required type="url" readOnly={existing} placeholder="http://127.0.0.1:8989" value={app.origin} onChange={(e) => setApp({ ...app, origin: e.target.value })} /></label>
        <label>{t("Application API key")}<input type="password" autoComplete="new-password" value={apiKey} onChange={(e) => setApiKey(e.target.value)} /></label>
        <label>{t("IndexLoom feed API key")}<input type="password" autoComplete="new-password" value={feedKey} onChange={(e) => setFeedKey(e.target.value)} /></label>
        <label><input type="checkbox" checked={app.enabled} onChange={(e) => setApp({ ...app, enabled: e.target.checked })} />{t("Enable application")}</label>
      </div><p>{t("Use a loopback HTTP origin or public HTTPS origin. Leave both keys blank to keep saved credentials.")}</p><button type="submit">{t("Save application")}</button></fieldset>
    </form>
    {existing ? <>
      <form key={app.id} onSubmit={(e) => { e.preventDefault(); void run(async () => { await request(base + "/profile", "PUT", profile); setMessage(t("Sync profile saved.")); }); }}>
        <fieldset disabled={busy}><legend>{t("Sync profile")}</legend><div className="appearance-controls">
          <label>{t("Sync mode")}<select value={profile.mode} onChange={(e) => setProfile({ ...profile, mode: e.target.value as Profile["mode"] })}><option value="Disabled">{t("Disabled")}</option><option value="AddOnly">{t("Add only")}</option><option value="FullSync">{t("Full sync")}</option></select></label>
          <label>{t("IndexLoom origin reachable by the application")}<input type="url" required={profile.mode !== "Disabled"} value={profile.feedOrigin ?? ""} onChange={(e) => setProfile({ ...profile, feedOrigin: e.target.value || null })} /></label>
          {(["indexerIds", "groupIds", "tags", "categories", "remoteTags"] as const).map((field) => <label key={field}>{t(({ indexerIds: "Indexer IDs", groupIds: "Group IDs", tags: "Indexer tags", categories: "Category IDs", remoteTags: "Application tag IDs" })[field])}<CommaInput initial={profile[field].join(",")} onChange={(value) => setProfile({ ...profile, [field]: field === "categories" || field === "remoteTags" ? split(value).map(Number) : split(value) })} /></label>)}
          <label>{t("Priority")}<input type="number" min={1} max={50} value={profile.priority} onChange={(e) => setProfile({ ...profile, priority: Number(e.target.value) })} /></label>
        </div><p>{t("Separate selectors with commas. Empty selectors include all indexers. Full sync updates and removes only entries recorded as owned by this integration.")}</p><button type="submit">{t("Save sync profile")}</button></fieldset>
      </form>
      <div className="appearance-controls">
        <button disabled={busy} onClick={() => { void run(async () => { await request(base + "/test", "POST"); setMessage(t("Connection test passed.")); }); }} type="button">{t("Test connection")}</button>
        <button disabled={busy} onClick={() => { void run(async () => { const result = await request<Record<string, unknown>>(base + "/sync", "POST"); setMessage(JSON.stringify(result)); }); }} type="button">{t("Sync saved profile")}</button>
        <button disabled={busy} onClick={() => { if (window.confirm(t("Delete this integration? Remote entries will remain in the application."))) void run(async () => { await request(base, "DELETE"); setApp(blankApp); setProfile(blankProfile); setApps(await request<App[]>("integrations")); setMessage(t("Application deleted.")); }); }} type="button">{t("Delete application")}</button>
      </div>
    </> : null}
    <p role="status">{busy ? t("Working…") : message}</p>{error ? <p role="alert" className="inline-error">{error}</p> : null}
  </div>;
}

function CommaInput({ initial, onChange }: { initial: string; onChange: (value: string) => void }) {
  const [text, setText] = useState(initial);
  return <input value={text} onChange={(event) => { setText(event.target.value); onChange(event.target.value); }} />;
}
