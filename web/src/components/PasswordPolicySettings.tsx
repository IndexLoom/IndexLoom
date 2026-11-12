import { type FormEvent, useEffect, useState } from "react";
import { t } from "../lib/i18n";
import {
  DEFAULT_PASSWORD_POLICY, fetchPasswordPolicy, passwordRuleHints, presetPolicy,
  type CountRule, type PasswordPolicy, type PasswordPreset,
} from "../lib/password-policy";

type ToggleKey = "minimumLengthEnabled" | "maximumLengthEnabled"
  | "rejectWhitespace" | "rejectUsername" | "rejectEmail"
  | "rejectCommon" | "rejectRepeated" | "rejectSequences" | "historyEnabled";
type CountKey = "uppercase" | "lowercase" | "digits" | "symbols";

const toggles: ReadonlyArray<{ key: ToggleKey; label: string; description: string }> = [
  { key: "rejectWhitespace", label: "Disallow spaces and whitespace", description: "Block spaces, tabs and line breaks." },
  { key: "rejectUsername", label: "Disallow username in password", description: "Case-insensitive username matching." },
  { key: "rejectEmail", label: "Disallow email in password", description: "Blocks the email address and its local part." },
  { key: "rejectCommon", label: "Block common passwords", description: "Rejects a built-in list of predictable passwords." },
  { key: "rejectRepeated", label: "Limit repeated characters", description: "Prevent long runs such as aaaa." },
  { key: "rejectSequences", label: "Block sequential characters", description: "Prevent sequences such as abcd and 4321." },
  { key: "historyEnabled", label: "Prevent password reuse", description: "Compare against recent Argon2id password hashes." },
];

const counts: ReadonlyArray<{ key: CountKey; label: string }> = [
  { key: "uppercase", label: "Uppercase letters" },
  { key: "lowercase", label: "Lowercase letters" },
  { key: "digits", label: "Digits" },
  { key: "symbols", label: "Special characters" },
];

function RuleRow({ label, description, enabled, onChange, children }: {
  label: string;
  description?: string;
  enabled: boolean;
  onChange: (value: boolean) => void;
  children?: React.ReactNode;
}) {
  return (
    <div className="password-policy-row">
      <label className="password-policy-toggle">
        <input type="checkbox" checked={enabled} onChange={(event) => onChange(event.target.checked)} />
        <span><strong>{t(label)}</strong>{description ? <small>{t(description)}</small> : null}</span>
      </label>
      {children}
    </div>
  );
}

export function PasswordPolicyEditor({ token }: { token: string }) {
  const [policy, setPolicy] = useState<PasswordPolicy | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  useEffect(() => {
    const abort = new AbortController();
    void fetchPasswordPolicy(abort.signal).then(setPolicy).catch((error: unknown) => {
      if (!abort.signal.aborted) setMessage(error instanceof Error ? error.message : "Could not load password policy");
    });
    return () => abort.abort();
  }, []);

  function updateToggle(key: ToggleKey, enabled: boolean) {
    setPolicy((current) => current ? { ...current, [key]: enabled } : current);
    setMessage("");
  }
  function updateNumeric(key: "minimumLength" | "maximumLength" | "maximumRepeated" | "sequenceLength" | "historyCount", next: number) {
    setPolicy((current) => current ? { ...current, [key]: next } : current);
    setMessage("");
  }
  function updateCount(key: CountKey, patch: Partial<CountRule>) {
    setPolicy((current) => current ? { ...current, [key]: { ...current[key], ...patch } } : current);
    setMessage("");
  }
  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!policy) return;
    setBusy(true);
    setMessage("");
    try {
      const response = await fetch("/api/v1/auth/password-policy/settings", {
        method: "PUT", cache: "no-store",
        headers: { Authorization: "Bearer " + token, "Content-Type": "application/json" },
        body: JSON.stringify(policy),
      });
      const payload = await response.json().catch(() => null) as (PasswordPolicy & { message?: string }) | null;
      if (!response.ok) throw new Error(payload?.message ?? "Could not save password policy");
      setPolicy(payload);
      setMessage("Password policy saved. Existing passwords are unchanged.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Could not save password policy");
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="password-policy-editor">
      <div className="setting-heading">
        <h2>{t("Password policy")}</h2>
        <p>{t("Enable individual requirements for newly created and changed local passwords. Current passwords, OIDC and API tokens are unaffected.")}</p>
      </div>
      {policy ? <form onSubmit={(event) => { void save(event); }}>
        <div className="password-policy-presets" aria-label={t("Password policy presets")}>
          {(["passphrase", "balanced", "strict"] as PasswordPreset[]).map((name) => (
            <button className="quiet-button" type="button" key={name} disabled={busy}
              onClick={() => { setPolicy(presetPolicy(name)); setMessage(""); }}>
              {t(({ passphrase: "Passphrase preset", balanced: "Balanced preset", strict: "Strict preset" })[name])}
            </button>
          ))}
          <button className="quiet-button" type="button" disabled={busy} onClick={() => { setPolicy({ ...DEFAULT_PASSWORD_POLICY }); setMessage(""); }}>
            {t("Restore default rules")}
          </button>
        </div>
        <h3>{t("Length requirements")}</h3>
        <RuleRow label="Minimum password length" description="Mandatory only while enabled." enabled={policy.minimumLengthEnabled}
          onChange={(checked) => updateToggle("minimumLengthEnabled", checked)}>
          <input aria-label={t("Minimum length in characters")} type="number" min={8} max={128} required
            value={policy.minimumLength} onChange={(event) => updateNumeric("minimumLength", Number(event.target.value))} />
        </RuleRow>
        <RuleRow label="Maximum password length" description="Hard security cap of 128 UTF-8 bytes always applies." enabled={policy.maximumLengthEnabled}
          onChange={(checked) => updateToggle("maximumLengthEnabled", checked)}>
          <input aria-label={t("Maximum length in characters")} type="number" min={8} max={128} required
            value={policy.maximumLength} onChange={(event) => updateNumeric("maximumLength", Number(event.target.value))} />
        </RuleRow>
        <h3>{t("Required character types")}</h3>
        {counts.map(({ key, label }) => (
          <RuleRow key={key} label={label} description={t("Minimum required count")} enabled={policy[key].enabled}
            onChange={(checked) => updateCount(key, { enabled: checked })}>
            <input aria-label={t(label) + " " + t("Minimum required count")} type="number" required min={1} max={32}
              value={policy[key].minimum} onChange={(event) => updateCount(key, { minimum: Number(event.target.value) })} />
          </RuleRow>
        ))}
        <h3>{t("Additional security requirements")}</h3>
        {toggles.map(({ key, label, description }) => (
          <RuleRow key={key} label={label} description={description} enabled={policy[key]} onChange={(checked) => updateToggle(key, checked)}>
            {key === "rejectRepeated" ? <input aria-label={t("Maximum consecutive repeats")} type="number" min={2} max={12}
              value={policy.maximumRepeated} onChange={(event) => updateNumeric("maximumRepeated", Number(event.target.value))} /> : null}
            {key === "rejectSequences" ? <input aria-label={t("Blocked sequence length")} type="number" min={3} max={12}
              value={policy.sequenceLength} onChange={(event) => updateNumeric("sequenceLength", Number(event.target.value))} /> : null}
            {key === "historyEnabled" ? <input aria-label={t("Previous passwords to remember")} type="number" min={1} max={10}
              value={policy.historyCount} onChange={(event) => updateNumeric("historyCount", Number(event.target.value))} /> : null}
          </RuleRow>
        ))}
        <p className="muted">{t("Even with every optional rule off, the server requires at least 8 characters, a maximum of 128 UTF-8 bytes, and no control characters.")}</p>
        <button className="primary-button" type="submit" disabled={busy}>{t(busy ? "Saving…" : "Save password policy")}</button>
      </form> : <p className="muted">{t("Loading password policy…")}</p>}
      {message ? <p className="notice" role="status">{t(message)}</p> : null}
    </section>
  );
}

export function PasswordRequirements({ value, username, email }: {
  value: string; username?: string; email?: string;
}) {
  const [policy, setPolicy] = useState<PasswordPolicy | null>(null);
  useEffect(() => {
    const abort = new AbortController();
    void fetchPasswordPolicy(abort.signal).then(setPolicy).catch(() => {});
    return () => abort.abort();
  }, []);
  if (!policy) return null;
  const list = passwordRuleHints(policy, value);
  if (policy.rejectUsername && username && username.length >= 3) list.push({
    text: t("Must not contain username"),
    satisfied: !value.toLowerCase().includes(username.toLowerCase()),
  });
  if (policy.rejectEmail && email && email.includes("@")) {
    const local = email.split("@")[0] ?? "";
    list.push({
      text: t("Must not contain email"),
      satisfied: !value.toLowerCase().includes(email.toLowerCase()) && (local.length < 3 || !value.toLowerCase().includes(local.toLowerCase())),
    });
  }
  return <div className="password-requirements" aria-label={t("Password requirements")}>
    <strong>{t("Password requirements")}</strong>
    <ul>{list.map((rule) => <li key={rule.text} className={value && rule.satisfied ? "satisfied" : ""}>
      {value && rule.satisfied ? "✓ " : "• "}{t(rule.text)}
    </li>)}</ul>
    <small>{t("The server also checks common passwords, repeated patterns and password history where enabled.")}</small>
  </div>;
}
