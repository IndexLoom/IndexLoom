import { type FormEvent, useEffect, useState } from "react";
import { useAdminSession } from "./AdminSession";

type MailTemplate = {
  kind: string;
  subject: string;
  textBody: string;
  htmlBody: string;
};
type MailEventSetting = { kind: string; enabled: boolean };
type MailPanel = { enabled: boolean; templates: MailTemplate[]; events: MailEventSetting[] };

async function api<T>(token: string, path: string, options: RequestInit = {}): Promise<T> {
  const response = await fetch(path, {
    ...options, cache: "no-store",
    headers: {
      Authorization: "Bearer " + token,
      "Content-Type": "application/json",
      ...options.headers,
    },
  });
  if (!response.ok) {
    const body = await response.json().catch(() => null) as { message?: string } | null;
    throw new Error(body?.message || "HTTP " + response.status);
  }
  if (response.status === 204) return undefined as T;
  return await response.json() as T;
}
const kinds: Record<string, string> = {
  "verify-email": "Email confirmation",
  "reset-password": "Password recovery",
  "login-alert": "Sign-in notification",
  test: "SMTP test",
};

export function EmailSettings() {
  const { token, config } = useAdminSession();
  const [panel, setPanel] = useState<MailPanel | null>(null);
  const [kind, setKind] = useState("verify-email");
  const [draft, setDraft] = useState<MailTemplate | null>(null);
  const [testTo, setTestTo] = useState("");
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("");

  useEffect(() => {
    const controller = new AbortController();
    void api<MailPanel>(token, "/api/v1/auth/mail", { signal: controller.signal })
      .then((value) => {
        if (controller.signal.aborted) return;
        setPanel(value);
        setDraft(value.templates.find((template) => template.kind === kind) ?? null);
      })
      .catch((error: unknown) => {
        if (!controller.signal.aborted)
          setStatus(error instanceof Error ? error.message : "Mail settings unavailable");
      });
    return () => controller.abort();
  }, [token]);

  function choose(next: string) {
    setKind(next);
    setDraft(panel?.templates.find((template) => template.kind === next) ?? null);
  }

  async function toggleEvent(event: MailEventSetting) {
    setBusy(true); setStatus("");
    try {
      await api<void>(token, "/api/v1/auth/mail/events/" + event.kind, {
        method: "PUT", body: JSON.stringify({ enabled: !event.enabled }),
      });
      setPanel((current) => current && ({
        ...current,
        events: current.events.map((entry) =>
          entry.kind === event.kind ? { ...entry, enabled: !entry.enabled } : entry),
      }));
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Could not save event settings");
    } finally { setBusy(false); }
  }

  async function saveTemplate(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!draft) return;
    setBusy(true); setStatus("");
    try {
      const saved = await api<MailTemplate>(token, "/api/v1/auth/mail/templates/" + draft.kind, {
        method: "PUT", body: JSON.stringify({
          subject: draft.subject, textBody: draft.textBody, htmlBody: draft.htmlBody,
        }),
      });
      setPanel((current) => current && ({
        ...current, templates: current.templates.map((value) => value.kind === saved.kind ? saved : value),
      }));
      setStatus("Template saved.");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Could not save template");
    } finally { setBusy(false); }
  }

  async function sendTest(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true); setStatus("");
    try {
      await api<void>(token, "/api/v1/auth/mail/test", {
        method: "POST", body: JSON.stringify({ to: testTo }),
      });
      setStatus("Test message sent to SMTP transport.");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "SMTP test failed");
    } finally { setBusy(false); }
  }

  return (
    <section className="appearance-content email-settings" role="tabpanel">
      <div className="setting-heading">
        <h2>Email delivery</h2>
        <p>SMTP delivery can be disabled independently of local and OIDC authentication.</p>
      </div>
      <dl className="capability-facts">
        <div><dt>SMTP</dt><dd>{panel?.enabled ? "Connected configuration" : "Disabled"}</dd></div>
        <div><dt>Authentication mode</dt><dd>{config?.mode}</dd></div>
      </dl>
      {!panel?.enabled ? (
        <p className="notice">Email is off. Configure INDEXLOOM_MAIL_MODE=smtp and SMTP credentials on the server to enable delivery. Templates can still be edited.</p>
      ) : null}
      <div className="setting-heading"><h2>Notification events</h2></div>
      <div className="notification-events">
        {panel?.events.map((event) => (
          <label key={event.kind} className="notification-event">
            <div><strong>{kinds[event.kind] ?? event.kind}</strong><small>{event.enabled ? "Enabled" : "Disabled"}</small></div>
            <input type="checkbox" disabled={busy || !panel.enabled} checked={event.enabled} onChange={() => { void toggleEvent(event); }} />
          </label>
        ))}
      </div>
      <div className="setting-heading">
        <h2>Notification templates</h2>
        <p>Fully customize the subject, plain-text body and optional HTML markup. Allowed variables: {"{{app}}"}, {"{{username}}"}, {"{{email}}"}, {"{{action_url}}"}, {"{{event}}"}, {"{{time}}"}.</p>
      </div>
      {draft ? (
        <form className="mail-template-form" onSubmit={(event) => { void saveTemplate(event); }}>
          <label>Template
            <select value={kind} onChange={(event) => choose(event.target.value)}>
              {Object.entries(kinds).map(([id, title]) => <option key={id} value={id}>{title}</option>)}
            </select>
          </label>
          <label>Subject
            <input maxLength={250} required value={draft.subject}
              onChange={(event) => setDraft((value) => value && ({ ...value, subject: event.target.value }))} />
          </label>
          <label>Plain text
            <textarea rows={8} maxLength={20000} value={draft.textBody}
              onChange={(event) => setDraft((value) => value && ({ ...value, textBody: event.target.value }))} />
          </label>
          <label>HTML (optional)
            <textarea rows={10} spellCheck={false} maxLength={50000} value={draft.htmlBody}
              onChange={(event) => setDraft((value) => value && ({ ...value, htmlBody: event.target.value }))} />
          </label>
          <p className="muted">HTML is rendered only inside outgoing emails, not inside the admin page. Include a plain-text equivalent for clients that block HTML.</p>
          <button className="primary-button" disabled={busy} type="submit">Save template</button>
        </form>
      ) : <p className="muted">Loading templates…</p>}
      {panel?.enabled ? (
        <>
          <div className="setting-heading"><h2>Send SMTP test</h2></div>
          <form className="mail-test-form" onSubmit={(event) => { void sendTest(event); }}>
            <label>Recipient <input type="email" required value={testTo} onChange={(event) => setTestTo(event.target.value)} /></label>
            <button className="quiet-button" disabled={busy} type="submit">Send test email</button>
          </form>
        </>
      ) : null}
      {status ? <p className="notice" role="status">{status}</p> : null}
    </section>
  );
}
