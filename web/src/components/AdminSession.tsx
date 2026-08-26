import { type FormEvent, useCallback, useState } from "react";

const ADMIN_SESSION_KEY = "indexloom.admin-token.v1";

export function useAdminSession() {
  const [token, setToken] = useState(
    () => sessionStorage.getItem(ADMIN_SESSION_KEY) ?? "",
  );
  const acceptToken = useCallback((value: string) => {
    sessionStorage.setItem(ADMIN_SESSION_KEY, value);
    setToken(value);
  }, []);
  const clearToken = useCallback(() => {
    sessionStorage.removeItem(ADMIN_SESSION_KEY);
    setToken("");
  }, []);
  return { token, acceptToken, clearToken };
}

export function AdminTokenGate({
  onSubmit,
}: {
  onSubmit: (token: string) => void;
}) {
  const [value, setValue] = useState("");

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (value.length >= 32) {
      onSubmit(value);
    }
  }

  return (
    <section
      className="empty-view credential-gate"
      aria-labelledby="credential-heading"
    >
      <p className="eyebrow">Protected workspace</p>
      <h1 id="credential-heading">Administrator token</h1>
      <p>
        Unlock search and configuration for this tab. Your token is stored in
        session storage and disappears when the tab closes.
      </p>
      <form className="credential-gate-form" onSubmit={submit}>
        <label htmlFor="admin-token">Administrator token</label>
        <input
          autoComplete="current-password"
          autoFocus
          id="admin-token"
          minLength={32}
          onChange={(event) => setValue(event.target.value)}
          required
          type="password"
          value={value}
        />
        <button className="primary-button" type="submit">
          Unlock workspace
        </button>
      </form>
    </section>
  );
}
