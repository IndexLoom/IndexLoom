import {
  createContext, type FormEvent, type ReactNode,
  useCallback, useContext, useEffect, useMemo, useState,
} from "react";

type AuthMode = "token" | "local" | "oidc" | "hybrid" | "disabled";
type AuthConfig = {
  mode: AuthMode;
  localEnabled: boolean;
  oidcEnabled: boolean;
  mailEnabled: boolean;
  emailVerificationRequired: boolean;
  bootstrapAvailable: boolean;
  oidcIssuer: string | null;
};
type AuthSession = {
  accessToken: string;
  expiresAt: number;
  user: { id: string; username: string; email: string; emailVerified: boolean };
};
type AuthContextType = {
  token: string;
  acceptToken: (token: string, remember?: boolean) => void;
  clearToken: () => void;
  config: AuthConfig | null;
};

const KEY = "indexloom.admin-token.v1";
const PERSISTENT_KEY = "indexloom.login-session.v1";
const OIDC_STATE = "indexloom.oidc-state.v1";
const OIDC_REMEMBER = "indexloom.oidc-remember.v1";
const Context = createContext<AuthContextType | null>(null);

function savedToken(): string {
  try {
    return sessionStorage.getItem(KEY) ?? localStorage.getItem(PERSISTENT_KEY) ?? "";
  } catch { return ""; }
}

type Panel = "login" | "bootstrap" | "token" | "recover" | "reset";

async function postJson<T>(route: string, body: unknown): Promise<T> {
  const response = await fetch(route, {
    method: "POST",
    cache: "no-store",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    const parsed = await response.json().catch(() => null) as { message?: string } | null;
    throw new Error(parsed?.message || "Request failed (" + response.status + ")");
  }
  if (response.status === 204 || response.status === 202) return undefined as T;
  return await response.json() as T;
}

export function AuthenticationProvider({ children }: { children: ReactNode }) {
  const [token, setToken] = useState(savedToken);
  const [config, setConfig] = useState<AuthConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  const acceptToken = useCallback((next: string, remember = false) => {
    try {
      sessionStorage.removeItem(KEY);
      localStorage.removeItem(PERSISTENT_KEY);
      if (remember) localStorage.setItem(PERSISTENT_KEY, next);
      else sessionStorage.setItem(KEY, next);
    } catch { /* Private browser mode: keep token in memory only. */ }
    setConfig((current) =>
      current?.bootstrapAvailable ? { ...current, bootstrapAvailable: false } : current
    );
    setToken(next);
  }, []);

  const clearToken = useCallback(() => {
    const current = token;
    if (current && current !== "disabled") {
      void fetch("/api/v1/auth/logout", {
        method: "POST",
        headers: { Authorization: "Bearer " + current },
      }).catch(() => {});
    }
    try {
      sessionStorage.removeItem(KEY);
      localStorage.removeItem(PERSISTENT_KEY);
    } catch { /* Storage can be unavailable. */ }
    setToken("");
  }, [token]);

  const loadConfig = useCallback(async () => {
    setLoading(true);
    try {
      const response = await fetch("/api/v1/auth/config", {
        cache: "no-store", headers: { Accept: "application/json" },
      });
      if (!response.ok) throw new Error("Authentication configuration unavailable");
      const next = await response.json() as AuthConfig;
      setConfig(next);
      setError("");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Cannot connect to server");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { void loadConfig(); }, [loadConfig]);

  // Reject credentials left over from a different server authorization mode.
  useEffect(() => {
    if (!config || config.mode === "disabled" || !token) return;
    const controller = new AbortController();
    void fetch("/api/v1/auth/me", {
      cache: "no-store",
      headers: { Authorization: "Bearer " + token },
      signal: controller.signal,
    }).then((response) => {
      if (response.status !== 401 || controller.signal.aborted) return;
      try {
        sessionStorage.removeItem(KEY);
        localStorage.removeItem(PERSISTENT_KEY);
      } catch { /* Browser storage may be disabled. */ }
      setToken("");
    }).catch(() => { /* Keep the UI available during offline mode. */ });
    return () => controller.abort();
  }, [config, token]);

  // Complete OIDC on the same-origin callback without leaking the code to app state or logs.
  useEffect(() => {
    if (!config) return;
    const url = new URL(window.location.href);
    const code = url.searchParams.get("code");
    const state = url.searchParams.get("state");
    const verifyToken = url.searchParams.get("verify-email");
    const oidcError = url.searchParams.get("error");
    if (!code && !verifyToken && !oidcError) return;
    url.searchParams.delete("code");
    url.searchParams.delete("state");
    url.searchParams.delete("error");
    url.searchParams.delete("error_description");
    url.searchParams.delete("verify-email");
    window.history.replaceState(null, "", url.pathname + url.search + url.hash);
    if (verifyToken) {
      void postJson<void>("/api/v1/auth/email/verify", { token: verifyToken })
        .then(() => setNotice("Email confirmed. You can sign in."))
        .catch(() => setError("The verification link is invalid or expired."));
    }
    if (oidcError) {
      setError("Identity provider rejected the sign-in.");
      return;
    }
    if (!code || !state) return;
    const expected = sessionStorage.getItem(OIDC_STATE);
    const rememberOidc = sessionStorage.getItem(OIDC_REMEMBER) === "true";
    sessionStorage.removeItem(OIDC_STATE);
    sessionStorage.removeItem(OIDC_REMEMBER);
    if (!expected || expected !== state) {
      setError("Invalid OIDC state. Start sign-in again.");
      return;
    }
    setLoading(true);
    void postJson<AuthSession>("/api/v1/auth/oidc/complete", {
      code, state, rememberMe: rememberOidc,
    }).then((session) => {
      acceptToken(session.accessToken, rememberOidc);
      setNotice("Signed in as " + session.user.username + ".");
    }).catch((reason: unknown) => {
      setError(reason instanceof Error ? reason.message : "OIDC sign-in failed");
    }).finally(() => setLoading(false));
  }, [acceptToken, config]);

  const context = useMemo<AuthContextType>(() => ({
    token: config?.mode === "disabled" ? "disabled" : token,
    acceptToken, clearToken, config,
  }), [acceptToken, clearToken, config, token]);

  if (loading) return (
    <main className="auth-page" aria-busy="true">
      <div className="auth-brand">IL <strong>IndexLoom</strong></div>
      <p>Connecting to workspace…</p>
    </main>
  );
  if (!config) return (
    <main className="auth-page">
      <div className="auth-card">
        <h1>Connection unavailable</h1>
        <p>{error || "Check the server connection."}</p>
        <button className="primary-button" onClick={() => { void loadConfig(); }} type="button">Retry</button>
      </div>
    </main>
  );
  if (!context.token) return (
    <Context.Provider value={context}>
      <AuthenticationGate
        config={config}
        error={error}
        notice={notice}
        onLogin={acceptToken}
        onNotice={setNotice}
        onError={setError}
      />
    </Context.Provider>
  );
  return <Context.Provider value={context}>{children}</Context.Provider>;
}

export function useAdminSession() {
  const context = useContext(Context);
  if (!context) throw new Error("AuthenticationProvider is missing");
  return context;
}

function AuthenticationGate({ config, error, notice, onLogin, onNotice, onError }: {
  config: AuthConfig;
  error: string;
  notice: string;
  onLogin: (token: string, remember?: boolean) => void;
  onNotice: (value: string) => void;
  onError: (value: string) => void;
}) {
  const [panel, setPanel] = useState<Panel>(() =>
    new URLSearchParams(window.location.search).has("reset-password")
      ? "reset" : config.mode === "token" ? "token" : config.bootstrapAvailable ? "bootstrap" : "login"
  );
  const [login, setLogin] = useState("");
  const [password, setPassword] = useState("");
  const [email, setEmail] = useState("");
  const [username, setUsername] = useState("");
  const [operatorToken, setOperatorToken] = useState("");
  const [remember, setRemember] = useState(false);
  const [busy, setBusy] = useState(false);
  const [localError, setLocalError] = useState("");
  const [resetToken] = useState(() => new URLSearchParams(window.location.search).get("reset-password") ?? "");

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    setLocalError("");
    try {
      if (panel === "token") {
        const response = await fetch("/api/v1/auth/me", {
          cache: "no-store",
          headers: { Authorization: "Bearer " + operatorToken },
        });
        if (!response.ok) throw new Error("Incorrect administrator token");
        onLogin(operatorToken, remember);
      } else if (panel === "login") {
        const session = await postJson<AuthSession>("/api/v1/auth/login", { login, password, rememberMe: remember });
        onLogin(session.accessToken, remember);
      } else if (panel === "bootstrap") {
        const response = await fetch("/api/v1/auth/bootstrap", {
          method: "POST",
          headers: { Authorization: "Bearer " + operatorToken, "Content-Type": "application/json" },
          body: JSON.stringify({ username, email, password }),
        });
        if (!response.ok) {
          const result = await response.json().catch(() => null) as { message?: string } | null;
          throw new Error(result?.message || "Setup failed");
        }
        const session = await response.json() as AuthSession;
        onLogin(session.accessToken, remember);
      } else if (panel === "recover") {
        await postJson<void>("/api/v1/auth/email/request-reset", { email });
        onNotice("If the account exists, a recovery link has been sent.");
        setPanel("login");
      } else if (panel === "reset") {
        await postJson<void>("/api/v1/auth/email/reset", { token: resetToken, password });
        const url = new URL(window.location.href);
        url.searchParams.delete("reset-password");
        window.history.replaceState(null, "", url.pathname + url.search + url.hash);
        onNotice("Password updated. You can sign in.");
        setPanel("login");
      }
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : "Authentication failed";
      setLocalError(message);
      onError(message);
    } finally {
      setBusy(false);
    }
  }

  async function oidc() {
    setBusy(true);
    setLocalError("");
    try {
      const result = await postJson<{ authorizationUrl: string; state: string }>("/api/v1/auth/oidc/start", {});
      const target = new URL(result.authorizationUrl);
      if (target.protocol !== "https:") throw new Error("Identity provider must use HTTPS");
      sessionStorage.setItem(OIDC_STATE, result.state);
      sessionStorage.setItem(OIDC_REMEMBER, remember ? "true" : "false");
      window.location.assign(target.href);
    } catch (reason) {
      setLocalError(reason instanceof Error ? reason.message : "OIDC is unavailable");
      setBusy(false);
    }
  }

  return (
    <main className="auth-page">
      <div className="auth-card">
        <header className="auth-header">
          <div className="auth-brand"><span>IL</span><strong>IndexLoom</strong></div>
          <p className="eyebrow">Private workspace</p>
          <h1>{panel === "bootstrap" ? "Set up your administrator" : panel === "reset" ? "New password" : panel === "recover" ? "Recover access" : "Welcome back"}</h1>
          <p>{panel === "bootstrap"
            ? "Initialize local login with the independent operator secret. This can only be done once."
            : panel === "token"
              ? "Sign in with your administrator access token."
              : panel === "recover" ? "Enter your account email to request a password reset."
                : panel === "reset" ? "Create a new password with at least 12 characters."
                  : "Sign in to your indexers, searches and settings."}</p>
        </header>
        <form className="auth-form" onSubmit={(event) => { void submit(event); }}>
          {panel === "bootstrap" ? (
            <>
              <label>Username <input autoComplete="username" minLength={3} maxLength={48} required value={username} onChange={(event) => setUsername(event.target.value)} /></label>
              <label>Email <input autoComplete="email" type="email" required value={email} onChange={(event) => setEmail(event.target.value)} /></label>
            </>
          ) : null}
          {panel === "login" ? (
            <label>Username or email <input autoComplete="username" required value={login} onChange={(event) => setLogin(event.target.value)} /></label>
          ) : null}
          {panel === "recover" ? (
            <label>Email <input autoComplete="email" type="email" required value={email} onChange={(event) => setEmail(event.target.value)} /></label>
          ) : null}
          {panel === "bootstrap" || panel === "token" ? (
            <label>Operator token <input autoComplete="off" type="password" minLength={32} required value={operatorToken} onChange={(event) => setOperatorToken(event.target.value)} /></label>
          ) : null}
          {panel === "login" || panel === "bootstrap" || panel === "reset" ? (
            <label>Password <input autoComplete={panel === "login" ? "current-password" : "new-password"} type="password" minLength={panel === "login" ? 1 : 12} required value={password} onChange={(event) => setPassword(event.target.value)} /></label>
          ) : null}
          {panel === "login" || panel === "token" || panel === "bootstrap" ? (
            <label className="auth-remember"><input type="checkbox" checked={remember} onChange={(event) => setRemember(event.target.checked)} />Keep me signed in on this device</label>
          ) : null}
          <button className="primary-button" disabled={busy} type="submit">
            {busy ? "Working…" : panel === "bootstrap" ? "Create administrator" : panel === "recover" ? "Send recovery link" : panel === "reset" ? "Update password" : "Sign in"}
          </button>
        </form>
        {config.oidcEnabled && panel !== "bootstrap" && panel !== "reset" ? (
          <button className="auth-oidc" disabled={busy} onClick={() => { void oidc(); }} type="button">
            Continue with OpenID Connect
          </button>
        ) : null}
        <nav className="auth-actions" aria-label="Sign-in options">
          {panel !== "login" && config.localEnabled && !config.bootstrapAvailable && panel !== "reset" ? (
            <button type="button" onClick={() => setPanel("login")}>Password login</button>
          ) : null}
          {panel !== "token" && (config.mode === "token" || config.mode === "hybrid") ? (
            <button type="button" onClick={() => setPanel("token")}>Administrator token</button>
          ) : null}
          {panel === "login" && config.mailEnabled ? (
            <button type="button" onClick={() => setPanel("recover")}>Forgot password?</button>
          ) : null}
          {panel === "recover" ? (
            <button type="button" onClick={() => setPanel("login")}>Back to sign in</button>
          ) : null}
        </nav>
        {localError || error ? <p className="auth-error" role="alert">{localError || error}</p> : null}
        {notice ? <p className="auth-notice" role="status">{notice}</p> : null}
      </div>
      <footer className="auth-footer">IndexLoom · self-hosted indexer workspace</footer>
    </main>
  );
}

/** Fallback for a workspace that explicitly cleared its token. */
export function AdminTokenGate({ onSubmit }: { onSubmit: (token: string) => void }) {
  const [value, setValue] = useState("");
  return (
    <section className="empty-view credential-gate">
      <h1>Session expired</h1>
      <p>Sign in again to continue.</p>
      <form className="credential-gate-form" onSubmit={(event) => {
        event.preventDefault();
        if (value.trim()) onSubmit(value.trim());
      }}>
        <label htmlFor="admin-token">Access token</label>
        <input autoComplete="current-password" id="admin-token" value={value} type="password" onChange={(event) => setValue(event.target.value)} required />
        <button className="primary-button" type="submit">Unlock</button>
      </form>
    </section>
  );
}
