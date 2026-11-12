import { t } from "../lib/i18n";
import { type FormEvent, useCallback, useEffect, useState } from "react";
import { useAdminSession } from "./AdminSession";

type Status = { enabled: boolean; recoveryCodesRemaining: number };
type Setup = { secret: string; otpauthUri: string; expiresInSeconds: number };
type Confirm = { recoveryCodes: string[] };

async function api<T>(token: string, path: string, data?: object): Promise<T> {
  const response = await fetch(path, {
    method: data ? "POST" : "GET",
    cache: "no-store",
    headers: {
      Authorization: "Bearer " + token,
      "Content-Type": "application/json",
    },
    ...(data ? { body: JSON.stringify(data) } : {}),
  });
  if (!response.ok) {
    const parsed = await response.json().catch(() => null) as { message?: string } | null;
    throw new Error(parsed?.message || "Request failed (" + response.status + ")");
  }
  if (response.status === 204) return undefined as T;
  return await response.json() as T;
}

export function TotpSettings() {
  const { token, clearToken } = useAdminSession();
  const [status, setStatus] = useState<Status | null>(null);
  const [setup, setSetup] = useState<Setup | null>(null);
  const [backupCodes, setBackupCodes] = useState<string[]>([]);
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");

  const refresh = useCallback(async () => {
    const next = await api<Status>(token, "/api/v1/auth/totp");
    setStatus(next);
  }, [token]);

  useEffect(() => {
    void refresh().catch((error: unknown) => {
      setMessage(error instanceof Error ? error.message : "Authenticator status unavailable.");
    });
  }, [refresh]);

  async function start(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true); setMessage("");
    try {
      const result = await api<Setup>(token, "/api/v1/auth/totp/start", { password });
      setSetup(result);
      setPassword("");
      setCode("");
      setMessage("Add this account to your authenticator. The enrollment expires after 10 minutes.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Could not start enrollment.");
    } finally { setBusy(false); }
  }

  async function confirm(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true); setMessage("");
    try {
      const result = await api<Confirm>(token, "/api/v1/auth/totp/confirm", { code });
      setBackupCodes(result.recoveryCodes);
      setSetup(null); setCode("");
      setStatus({ enabled: true, recoveryCodesRemaining: result.recoveryCodes.length });
      setMessage("Authenticator enabled. Your previous sessions were revoked. Save these recovery codes now; they will not be displayed again.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Authenticator verification failed.");
    } finally { setBusy(false); }
  }

  async function disable(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!window.confirm("Disable two-factor authentication and revoke all sessions?")) return;
    setBusy(true); setMessage("");
    try {
      await api<void>(token, "/api/v1/auth/totp/disable", { password, code });
      setPassword(""); setCode("");
      clearToken();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Could not disable two-factor authentication.");
    } finally { setBusy(false); }
  }

  async function copyCodes() {
    try {
      await navigator.clipboard.writeText(backupCodes.join("\n"));
      setMessage("Recovery codes copied. Store them in a secure password manager.");
    } catch {
      setMessage("Clipboard unavailable. Copy the recovery codes from the list manually.");
    }
  }

  return (
    <section className="totp-settings" aria-label={t("Two-factor authentication")}>
      <div className="setting-heading">
        <h2>{t("Two-factor authentication")}</h2>
        <p>Use one-time codes from an authenticator app with SHA-256 TOTP (30-second intervals), plus offline recovery codes.</p>
      </div>

      {status ? <dl className="capability-facts">
        <div><dt>{t("Authenticator")}</dt><dd>{status.enabled ? "Enabled" : "Not enabled"}</dd></div>
        <div><dt>{t("Recovery codes remaining")}</dt><dd>{status.recoveryCodesRemaining}</dd></div>
      </dl> : null}

      {backupCodes.length ? (
        <div className="totp-recovery">
          <h3>{t("Save your recovery codes")}</h3>
          <p>{t("Each code is valid once. This is your only opportunity to see them. Do not save them in a public note or screenshot.")}</p>
          <div className="totp-recovery-grid">
            {backupCodes.map((value) => <code key={value}>{value}</code>)}
          </div>
          <div className="user-actions">
            <button className="quiet-button" type="button" onClick={() => { void copyCodes(); }}>{t("Copy codes")}</button>
            <button className="primary-button" type="button" onClick={() => {
              setBackupCodes([]);
              clearToken();
            }}>{t("I've saved my codes — sign in again")}</button>
          </div>
        </div>
      ) : !status?.enabled && setup ? (
        <>
          <div className="totp-provisioning">
            <strong>{t("Authenticator setup key")}</strong>
            <code>{setup.secret}</code>
            <p>{t("Enter the key manually in your authenticator app, or open the provisioning link on your phone.")}</p>
            <a href={setup.otpauthUri}>{t("Open in authenticator")}</a>
          </div>
          <form className="auth-form" onSubmit={(event) => { void confirm(event); }}>
            <label>{t("Six-digit code from the app")}{" "}<input type="text" inputMode="numeric" autoComplete="one-time-code" pattern="[0-9]{6}"
                minLength={6} maxLength={6} value={code} required
                onChange={(event) => setCode(event.target.value)} />
            </label>
            <button className="primary-button" type="submit" disabled={busy}>{t("Enable authenticator")}</button>
          </form>
          <button className="quiet-button" type="button" onClick={() => {
            setSetup(null); setCode("");
          }}>{t("Cancel enrollment")}</button>
        </>
      ) : status?.enabled ? (
        <form className="auth-form" onSubmit={(event) => { void disable(event); }}>
          <p className="muted">{t("To disable two-factor authentication, confirm your password and enter a current authenticator or one-time recovery code.")}</p>
          <label>{t("Current password")}{" "}<input type="password" autoComplete="current-password" value={password} required
              onChange={(event) => setPassword(event.target.value)} />
          </label>
          <label>{t("Authenticator or recovery code")}{" "}<input type="text" autoComplete="one-time-code" value={code} required maxLength={48}
              onChange={(event) => setCode(event.target.value)} />
          </label>
          <button className="quiet-button" type="submit" disabled={busy}>{t("Disable two-factor authentication")}</button>
        </form>
      ) : (
        <form className="auth-form" onSubmit={(event) => { void start(event); }}>
          <label>{t("Current password")}{" "}<input type="password" autoComplete="current-password" value={password} required
              onChange={(event) => setPassword(event.target.value)} />
          </label>
          <button className="primary-button" type="submit" disabled={busy}>{t("Set up authenticator")}</button>
        </form>
      )}

      {message ? <p className="notice" role="status">{message}</p> : null}
    </section>
  );
}
