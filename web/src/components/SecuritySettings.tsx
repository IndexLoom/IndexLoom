import { type FormEvent, useEffect, useState } from "react";
import { useAdminSession } from "./AdminSession";

type User = {
  id: string;
  username: string;
  email: string;
  emailVerified: boolean;
  hasPassword: boolean;
  hasOidc: boolean;
  disabled: boolean;
};
type Me = { user: User | null; method: string };

async function request<T>(token: string, path: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(path, {
    cache: "no-store",
    ...init,
    headers: {
      Authorization: "Bearer " + token,
      "Content-Type": "application/json",
      ...init.headers,
    },
  });
  if (!response.ok) {
    const error = await response.json().catch(() => null) as { message?: string } | null;
    throw new Error(error?.message ?? "HTTP " + response.status);
  }
  if (response.status === 204) return undefined as T;
  return await response.json() as T;
}

export function SecuritySettings() {
  const { token, config, clearToken } = useAdminSession();
  const [me, setMe] = useState<Me | null>(null);
  const [users, setUsers] = useState<User[]>([]);
  const [loading, setLoading] = useState(true);
  const [feedback, setFeedback] = useState("");
  const [newUsername, setNewUsername] = useState("");
  const [newEmail, setNewEmail] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [currentPassword, setCurrentPassword] = useState("");
  const [replacement, setReplacement] = useState("");
  const [busy, setBusy] = useState(false);

  async function refresh() {
    if (!token) return;
    setLoading(true);
    try {
      const [current, members] = await Promise.all([
        request<Me>(token, "/api/v1/auth/me"),
        request<User[]>(token, "/api/v1/auth/users"),
      ]);
      setMe(current);
      setUsers(members);
    } catch (error) {
      setFeedback(error instanceof Error ? error.message : "Could not load accounts.");
    } finally {
      setLoading(false);
    }
  }
  useEffect(() => { void refresh(); }, [token]);

  async function addUser(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true); setFeedback("");
    try {
      const user = await request<User>(token, "/api/v1/auth/users", {
        method: "POST",
        body: JSON.stringify({ username: newUsername, email: newEmail, password: newPassword }),
      });
      setFeedback(user.emailVerified
        ? "Account created successfully."
        : "Account created. Verification email requested.");
      setNewUsername(""); setNewEmail(""); setNewPassword("");
      await refresh();
    } catch (error) {
      setFeedback(error instanceof Error ? error.message : "Account creation failed.");
    } finally { setBusy(false); }
  }

  async function toggleUser(user: User) {
    if (!window.confirm((user.disabled ? "Enable " : "Disable ") + user.username + "?")) return;
    setBusy(true); setFeedback("");
    try {
      await request<User>(token, "/api/v1/auth/users/" + encodeURIComponent(user.id) + "/disabled", {
        method: "PUT", body: JSON.stringify({ disabled: !user.disabled }),
      });
      await refresh();
    } catch (error) {
      setFeedback(error instanceof Error ? error.message : "Cannot change account state.");
    } finally { setBusy(false); }
  }

  async function changePassword(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true); setFeedback("");
    try {
      await request<void>(token, "/api/v1/auth/password", {
        method: "POST",
        body: JSON.stringify({ login: currentPassword, password: replacement }),
      });
      setCurrentPassword(""); setReplacement("");
      setFeedback("Password changed. All sessions revoked; please sign in again.");
      window.setTimeout(clearToken, 200);
    } catch (error) {
      setFeedback(error instanceof Error ? error.message : "Password change failed.");
    } finally { setBusy(false); }
  }

  return (
    <section className="appearance-content security-settings" role="tabpanel">
      <div className="setting-heading">
        <h2>Authentication</h2>
        <p>Credentials, sessions and account administration. Authorization rules are enforced server-side.</p>
      </div>
      <dl className="capability-facts">
        <div><dt>Mode</dt><dd>{config?.mode ?? "unknown"}</dd></div>
        <div><dt>Current identity</dt><dd>{me?.user?.username ?? (config?.mode === "disabled" ? "Anonymous" : "Operator token")}</dd></div>
        <div><dt>OIDC</dt><dd>{config?.oidcEnabled ? "Enabled" : "Off"}</dd></div>
        <div><dt>Mail</dt><dd>{config?.mailEnabled ? "Enabled" : "Off"}</dd></div>
        <div><dt>Email verification</dt><dd>{config?.emailVerificationRequired ? "Required" : "Optional"}</dd></div>
      </dl>
      <p className="muted">Mode and identity provider settings are controlled by environment variables. Disabling protection requires two explicit server settings and should never be used on an Internet-facing deployment.</p>
      {config?.mode !== "disabled" ? (
        <button className="quiet-button" onClick={clearToken} type="button">Revoke current session / sign out</button>
      ) : null}

      {me?.user?.hasPassword ? (
        <>
          <div className="setting-heading"><h2>Change password</h2></div>
          <form className="auth-form" onSubmit={(event) => { void changePassword(event); }}>
            <label>Current password <input type="password" autoComplete="current-password" required value={currentPassword} onChange={(event) => setCurrentPassword(event.target.value)} /></label>
            <label>New password <input type="password" autoComplete="new-password" minLength={12} required value={replacement} onChange={(event) => setReplacement(event.target.value)} /></label>
            <button className="primary-button" disabled={busy} type="submit">Update password and revoke sessions</button>
          </form>
        </>
      ) : null}

      {config?.mode !== "token" && config?.mode !== "disabled" ? (
        <>
          <div className="setting-heading">
            <h2>Users</h2>
            <p>All enabled accounts currently have administrator access. Only add people you trust.</p>
          </div>
          <div className="user-directory">
            {users.map((user) => (
              <div className="user-record" key={user.id}>
                <div>
                  <strong>{user.username}</strong>
                  <small>{user.email}</small>
                  <span className="muted">{user.hasOidc ? "OIDC" : "Password"} · {user.emailVerified ? "Email verified" : "Unverified"} · {user.disabled ? "Disabled" : "Active"}</span>
                </div>
                <div className="user-actions">
                  {!user.emailVerified && config?.mailEnabled ? (
                    <button className="quiet-button" type="button" disabled={busy} onClick={() => {
                      void request<void>(token, "/api/v1/auth/users/" + encodeURIComponent(user.id) + "/verify-email", { method: "POST" })
                        .then(() => setFeedback("Verification email sent."))
                        .catch((error: unknown) => setFeedback(error instanceof Error ? error.message : "Delivery failed"));
                    }}>Send verification</button>
                  ) : null}
                  <button className="quiet-button" type="button" disabled={busy || user.id === me?.user?.id} onClick={() => { void toggleUser(user); }}>
                    {user.disabled ? "Enable" : "Disable"}
                  </button>
                </div>
              </div>
            ))}
          </div>
          {config?.localEnabled ? (
            <>
              <div className="setting-heading"><h2>Create local account</h2></div>
              <form className="auth-form" onSubmit={(event) => { void addUser(event); }}>
                <label>Username <input value={newUsername} minLength={3} maxLength={48} required onChange={(event) => setNewUsername(event.target.value)} /></label>
                <label>Email <input type="email" value={newEmail} required onChange={(event) => setNewEmail(event.target.value)} /></label>
                <label>Temporary password <input type="password" minLength={12} autoComplete="new-password" value={newPassword} required onChange={(event) => setNewPassword(event.target.value)} /></label>
                <button className="primary-button" type="submit" disabled={busy}>Create account</button>
              </form>
            </>
          ) : null}
        </>
      ) : null}
      {feedback ? <p className="notice" role="status">{feedback}</p> : null}
      {loading ? <p className="muted">Loading account information…</p> : null}
    </section>
  );
}
