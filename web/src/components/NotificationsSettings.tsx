import { type FormEvent, useEffect, useRef, useState } from "react";
import { useAdminSession } from "./AdminSession";
import { translate, formatDateTime } from "../lib/i18n";
import { type Channel, type ChannelDraft, type Delivery, type NotificationEvent, type NotificationTemplate, type Provider, type TestResult, draftChannel, newChannel, notificationApi, providerNames, providers, secretFields } from "../lib/notifications";
const t = (key: string) => translate(key, undefined, "settings");
const secretLabels: Record<string, string> = { token: "API token", url: "Secret endpoint URL", bearer: "Bearer token (optional)", key: "Apprise configuration key", user: "Pushover user key" };
const eventLabels: Record<string, string> = {
  "system.startup": "Server startup", "auth.login": "Account sign-in", "automation.failed": "Automation failed", "automation.completed": "Automation completed", "indexer.test.failed": "Indexer test failed", "indexer.test.passed": "Indexer test passed",
};
const statusLabels: Record<string, string> = { pending: "Pending", sending: "Sending", retry: "Retry scheduled", sent: "Sent", failed: "Failed", cancelled: "Cancelled" };

export function NotificationsSettings() {
  const { token } = useAdminSession();
  const [channels, setChannels] = useState<Channel[]>([]);
  const [events, setEvents] = useState<NotificationEvent[]>([]);
  const [templates, setTemplates] = useState<NotificationTemplate[]>([]);
  const [deliveries, setDeliveries] = useState<Delivery[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [status, setStatus] = useState("");
  const [editing, setEditing] = useState<Channel | null>(null);
  const [draft, setDraft] = useState<ChannelDraft | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [scope, setScope] = useState("");
  const [event, setEvent] = useState("system.startup");
  const [template, setTemplate] = useState({ title: "{{title}}", body: "{{message}}" });
  const [preview, setPreview] = useState<{ title: string; body: string } | null>(null);
  const editor = useRef<HTMLInputElement>(null);
  function api<T>(path: string, init?: RequestInit) { return notificationApi<T>(token, path, init); }

  useEffect(() => {
    const controller = new AbortController();
    setLoading(true); setError("");
    void Promise.all([
      notificationApi<Channel[]>(token, "channels", { signal: controller.signal }),
      notificationApi<NotificationEvent[]>(token, "events", { signal: controller.signal }),
      notificationApi<NotificationTemplate[]>(token, "templates", { signal: controller.signal }),
      notificationApi<Delivery[]>(token, "deliveries", { signal: controller.signal }),
    ]).then(([c, e, ts, d]) => {
      if (!controller.signal.aborted) { setChannels(c); setEvents(e); setTemplates(ts); setDeliveries(d); }
    }).catch(() => { if (!controller.signal.aborted) setError("Could not load notifications"); })
      .finally(() => { if (!controller.signal.aborted) setLoading(false); });
    return () => controller.abort();
  }, [token]);
  useEffect(() => {
    const saved = templates.find((v) => v.scope === scope && v.event === event)
      ?? templates.find((v) => !v.scope && v.event === event);
    setTemplate({ title: saved?.title ?? "{{title}}", body: saved?.body ?? "{{message}}" });
    setPreview(null);
  }, [scope, event, templates]);
  useEffect(() => { if (draft) editor.current?.focus(); }, [draft !== null]);

  async function action(work: () => Promise<void>) {
    setBusy(true); setError(""); setStatus("");
    try { await work(); } catch (e) { setError(e instanceof Error ? e.message : "Notification request failed"); }
    finally { setBusy(false); }
  }
  const refreshLogs = async () => setDeliveries(await api<Delivery[]>("deliveries"));
  async function save(e: FormEvent) {
    e.preventDefault(); if (!draft) return;
    await action(async () => {
      const value = await api<Channel>("channels" + (editing ? "/" + editing.id : ""), { method: editing ? "PUT" : "POST", body: JSON.stringify(draft) });
      setChannels((prev) => editing ? prev.map((c) => c.id === value.id ? value : c) : [...prev, value]);
      setDraft(null); setEditing(null); setStatus("Notification channel saved"); await refreshLogs();
    });
  }
  function field<K extends keyof ChannelDraft>(key: K, value: ChannelDraft[K]) { setDraft((v) => v && ({ ...v, [key]: value })); }
  function configField(key: keyof ChannelDraft["config"], value: string | number | boolean | undefined) { setDraft((v) => v && ({ ...v, config: { ...v.config, [key]: value } })); }
  const templatePath = "templates/" + event + (scope ? "?channelId=" + encodeURIComponent(scope) : "");

  return <section className="appearance-content notifications-settings" aria-busy={busy || loading}>
    <header className="notification-heading"><div><h2>{t("External notifications")}</h2><p>{t("Send selected events to your own channels. Authentication emails are managed separately.")}</p></div>
      <button type="button" disabled={busy || loading} onClick={() => { setEditing(null); setDraft(newChannel()); }}>{t("Add channel")}</button></header>
    <p role="status" className="notification-notice">{loading ? t("Loading notifications…") : t(status)}</p>
    {error ? <p role="alert">{t(error)}</p> : null}
    {!loading && !channels.length ? <p className="muted">{t("No channels. External notifications are off until you add one.")}</p> : null}
    <ul className="notification-channel-list">
      {channels.map((c) => <li key={c.id}>
        <div className="notification-channel-name"><strong>{c.name}</strong><span className="muted">{providerNames[c.provider]} · {c.events.length} {t("events selected")}</span></div>
        <label className="notification-check"><input type="checkbox" checked={c.enabled} disabled={busy} onChange={() => void action(async () => {
          const updated = await api<Channel>("channels/" + c.id, { method: "PUT", body: JSON.stringify({ ...draftChannel(c), enabled: !c.enabled }) });
          setChannels((prev) => prev.map((v) => v.id === c.id ? updated : v)); await refreshLogs();
        })} /><span>{t(c.enabled ? "Enabled" : "Disabled")}</span></label>
        <div className="operations-actions">
          <button type="button" disabled={busy} onClick={() => { setEditing(c); setDraft(draftChannel(c)); }}>{t("Edit")}<span className="notification-sr"> {c.name}</span></button>
          <button type="button" disabled={busy} onClick={() => void action(async () => {
            const r = await api<TestResult>("channels/" + c.id + "/test", { method: "POST" });
            if (r.success) setStatus("Test accepted by provider"); else setError(t("Test failed") + ": " + t(r.summary));
            await refreshLogs();
          })}>{t("Send test")}<span className="notification-sr"> {c.name}</span></button>
          <button type="button" className="operations-danger" disabled={busy} onClick={() => setDeleting(c.id)}>{t("Delete")}<span className="notification-sr"> {c.name}</span></button>
        </div>
        {deleting === c.id ? <div className="notification-delete"><p>{t("Delete this channel and its delivery history?")}</p>
          <button type="button" disabled={busy} className="operations-danger" onClick={() => void action(async () => {
            await api("channels/" + c.id, { method: "DELETE" }); setChannels((v) => v.filter((x) => x.id !== c.id));
            if (editing?.id === c.id) { setEditing(null); setDraft(null); }
            if (scope === c.id) setScope("");
            setTemplates((v) => v.filter((x) => x.scope !== c.id)); setDeleting(null); await refreshLogs(); setStatus("Notification channel deleted");
          })}>{t("Confirm delete")}</button><button type="button" disabled={busy} onClick={() => setDeleting(null)}>{t("Cancel")}</button></div> : null}
      </li>)}
    </ul>
    {draft ? <form className="notification-editor" onSubmit={(e) => { void save(e); }}>
      <fieldset disabled={busy}><legend>{t(editing ? "Edit notification channel" : "New notification channel")}</legend>
        <div className="notification-grid">
          <label><span>{t("Channel name")}</span><input ref={editor} value={draft.name} required maxLength={120} onChange={(e) => field("name", e.target.value)} autoComplete="off" /></label>
          <label><span>{t("Provider")}</span><select value={draft.provider} onChange={(e) => {
            const next = newChannel(e.target.value as Provider); setDraft({ ...next, name: draft.name, enabled: draft.enabled, events: draft.events, revision: draft.revision });
          }}>{providers.map((p) => <option key={p} value={p}>{providerNames[p]}</option>)}</select></label>
          {draft.provider === "telegram" ? <>
            <label><span>{t("Telegram Bot API")}</span><select value={draft.config.apiBase ? "selfhosted" : "official"} onChange={(e) => {
              field("config", { ...draft.config, apiBase: e.target.value === "official" ? undefined : "http://127.0.0.1:8081", allowInsecureLocal: e.target.value !== "official" });
            }}><option value="official">{t("Official Telegram API")}</option><option value="selfhosted">{t("Self-hosted Bot API")}</option></select></label>
            <label><span>{t("Chat ID")}</span><input value={draft.config.chatId ?? ""} required={draft.enabled} maxLength={128} onChange={(e) => configField("chatId", e.target.value || undefined)} autoComplete="off" /></label>
            <label><span>{t("Thread ID (optional)")}</span><input type="number" min={1} step={1} value={draft.config.threadId ?? ""} onChange={(e) => configField("threadId", e.target.value ? Number(e.target.value) : undefined)} /></label>
          </> : null}
          {(["gotify", "ntfy"].includes(draft.provider) || (draft.provider === "telegram" && draft.config.apiBase !== undefined)) ? <label><span>{t("Public API base URL")}</span><input type="url" value={draft.config.apiBase ?? ""} required={draft.provider === "gotify"} maxLength={2048} placeholder={draft.provider === "ntfy" ? "https://ntfy.sh" : "https://"} onChange={(e) => configField("apiBase", e.target.value || undefined)} autoComplete="off" /></label> : null}
          {draft.provider === "ntfy" ? <label><span>{t("Topic")}</span><input value={draft.config.topic ?? ""} required={draft.enabled} maxLength={200} onChange={(e) => configField("topic", e.target.value || undefined)} /></label> : null}
          {["gotify", "ntfy", "pushover"].includes(draft.provider) ? <label><span>{t("Priority")}</span><input type="number" step={1} min={draft.provider === "pushover" ? -2 : draft.provider === "ntfy" ? 1 : 0} max={draft.provider === "pushover" ? 1 : draft.provider === "ntfy" ? 5 : 10} value={draft.config.priority ?? (draft.provider === "ntfy" ? 3 : draft.provider === "gotify" ? 5 : 0)} onChange={(e) => configField("priority", e.target.value ? Number(e.target.value) : undefined)} /></label> : null}
        </div>
        <div className="notification-secrets">
          <p className="muted">{t("Secrets are encrypted. Leave blank to keep a stored value; Clear removes it on save. Disable the channel before clearing required credentials.")}</p>
          {secretFields[draft.provider].map((key) => {
            const stored = editing?.provider === draft.provider && editing.storedSecrets.includes(key);
            const cleared = draft.secrets[key] === null;
            return <div className="notification-secret" key={key}><label><span>{t(secretLabels[key] ?? key)}</span><input type="password" maxLength={2048} autoComplete="new-password" spellCheck={false} value={draft.secrets[key] ?? ""}
              placeholder={cleared ? t("Will be cleared") : stored ? t("Stored · leave blank to keep") : t("Not configured")}
              onChange={(e) => { const next = { ...draft.secrets }; if (e.target.value) next[key] = e.target.value; else delete next[key]; field("secrets", next); }} /></label>
              <button type="button" onClick={() => { const next = { ...draft.secrets }; if (cleared) delete next[key]; else next[key] = null; field("secrets", next); }}>{t(cleared ? "Undo clear" : "Clear")}</button></div>;
          })}
        </div>
        <div className="operations-actions">
          <label className="notification-check"><input type="checkbox" checked={draft.enabled} onChange={(e) => field("enabled", e.target.checked)} /><span>{t("Enable channel")}</span></label>
          <label className="notification-check"><input type="checkbox" checked={draft.config.allowInsecureLocal ?? false} onChange={(e) => configField("allowInsecureLocal", e.target.checked)} /><span>{t("Allow HTTP for localhost or private IPs")}</span></label>
          {draft.provider === "telegram" ? <label className="notification-check"><input type="checkbox" checked={draft.config.disableNotification ?? false} onChange={(e) => configField("disableNotification", e.target.checked)} /><span>{t("Silent Telegram message")}</span></label> : null}
        </div>
        <fieldset><legend>{t("Events for this channel")}</legend><div className="notification-event-grid">{events.map((v) => <label className="notification-check" key={v.id}>
          <input type="checkbox" checked={draft.events.includes(v.id)} onChange={(e) => field("events", e.target.checked ? [...draft.events, v.id] : draft.events.filter((x) => x !== v.id))} /><span>{t(eventLabels[v.id] ?? v.id)}</span>
        </label>)}</div></fieldset>
        <div className="operations-actions"><button type="submit">{t("Save channel")}</button><button type="button" onClick={() => { setDraft(null); setEditing(null); }}>{t("Cancel")}</button></div>
      </fieldset>
    </form> : null}
    <details className="notification-templates"><summary>{t("Event templates")}</summary>
      <p className="muted">{t("Global templates apply to every channel. Channel overrides take precedence. Reset removes the override.")}</p>
      <form onSubmit={(e) => { e.preventDefault(); void action(async () => {
        await api(templatePath, { method: "PUT", body: JSON.stringify(template) }); setTemplates(await api<NotificationTemplate[]>("templates")); setStatus("Notification template saved");
      }); }}><fieldset disabled={busy || loading}>
        <div className="notification-grid"><label><span>{t("Template scope")}</span><select value={scope} onChange={(e) => setScope(e.target.value)}><option value="">{t("All channels")}</option>{channels.map((c) => <option key={c.id} value={c.id}>{c.name}</option>)}</select></label>
          <label><span>{t("Event")}</span><select value={event} onChange={(e) => setEvent(e.target.value)}>{events.map((v) => <option key={v.id} value={v.id}>{t(eventLabels[v.id] ?? v.id)}</option>)}</select></label></div>
        <label><span>{t("Notification title")}</span><input required maxLength={500} value={template.title} onChange={(e) => { setPreview(null); setTemplate({ ...template, title: e.target.value }); }} /></label>
        <label><span>{t("Notification body")}</span><textarea required maxLength={4000} rows={5} value={template.body} onChange={(e) => { setPreview(null); setTemplate({ ...template, body: e.target.value }); }} /></label>
        <p className="muted">{t("Literal placeholders; plain text only. Provider size limits may shorten messages.")}</p><code className="notification-placeholders">{"{{title}} {{message}} {{event}} {{time}} {{app}} {{indexer}} {{job}}"}</code>
        <div className="operations-actions"><button type="submit">{t("Save template")}</button><button type="button" onClick={() => void action(async () => {
          setPreview(await api<{ title: string; body: string }>("preview", { method: "POST", body: JSON.stringify({ event, ...template }) }));
        })}>{t("Preview")}</button><button type="button" onClick={() => void action(async () => {
          await api(templatePath, { method: "DELETE" }); setTemplates(await api<NotificationTemplate[]>("templates")); setStatus("Notification template reset");
        })}>{t("Reset template")}</button></div>
        {preview ? <output className="notification-preview"><strong>{preview.title}</strong><span>{preview.body}</span></output> : null}
      </fieldset></form>
    </details>
    <div className="notification-heading"><h3>{t("Delivery history")}</h3><button type="button" disabled={busy || loading} onClick={() => void action(refreshLogs)}>{t("Refresh")}</button></div>
    <p className="muted">{t("Five attempts with exponential backoff. History keeps up to 2,000 completed deliveries for 30 days. Provider acceptance does not confirm that a person read the message.")}</p>
    {!deliveries.length && !loading ? <p>{t("No deliveries yet.")}</p> : null}
    <ol className="notification-log">{deliveries.map((d) => <li key={d.id}>
      <div><strong>{channels.find((c) => c.id === d.channelId)?.name ?? t("Deleted channel")}</strong><span data-status={d.status} className="notification-status">{t(statusLabels[d.status] ?? d.status)}</span></div>
      <span>{d.isTest ? t("Test notification") : t(eventLabels[d.event] ?? d.event)}</span>
      <time dateTime={new Date(d.createdAt * 1000).toISOString()}>{formatDateTime(d.createdAt * 1000)}</time>
      <span className="muted">{t("Attempts")}: {d.attempts}{d.durationMs !== null ? " · " + d.durationMs + " ms" : ""}{d.status === "retry" ? " · " + t("Next attempt") + ": " + formatDateTime(d.nextAttempt * 1000) : ""}</span>
      {d.summary ? <span className="muted">{t(d.summary)}</span> : null}
    </li>)}</ol>
    {deliveries.length >= 100 ? <button type="button" disabled={busy} onClick={() => void action(async () => {
      const older = await api<Delivery[]>("deliveries?before=" + deliveries[deliveries.length - 1]!.id); setDeliveries((v) => [...v, ...older]);
    })}>{t("Load older deliveries")}</button> : null}
  </section>;
}
