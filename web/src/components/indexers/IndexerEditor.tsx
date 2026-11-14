import { type FormEvent, useState } from "react";
import { translate, categoryLabel, languageLabel, searchModeLabel, formatNumber } from "../../lib/i18n";
import { ApiError, type ConfigurationField, type ConfigurationValue, type FlareSolverrInstance,
  type Indexer, type IndexerGroup, type OutboundProxy, saveIndexerConfiguration, saveIndexerOrganization } from "../../lib/api";
const t = (message: string, variables?: Record<string, string | number>) => translate(message, variables, "indexers");

import { TrackerLoginPanel } from "./TrackerLoginPanel";

export function IndexerEditor({
  indexer,
  connectors,
  proxies,
  groups,
  token,
  onClose,
  onSaved,
  onUnauthorized,
}: {
  indexer: Indexer;
  connectors: FlareSolverrInstance[];
  proxies: OutboundProxy[];
  groups: IndexerGroup[];
  token: string;
  onClose: () => void;
  onSaved: (indexer: Indexer) => void;
  onUnauthorized: () => void;
}) {
  const [values, setValues] = useState<Record<string, ConfigurationValue>>(() =>
    initialValues(indexer),
  );
  const [userAgent, setUserAgent] = useState(indexer.userAgent ?? "");
  const [connectorId, setConnectorId] = useState(
    indexer.flaresolverrInstanceId ?? "",
  );
  const [displayName, setDisplayName] = useState(indexer.displayName);
  const [enabled, setEnabled] = useState(indexer.enabled);
  const [proxyId, setProxyId] = useState(indexer.proxyId ?? "");
  const [groupIds, setGroupIds] = useState<Set<string>>(
    () => new Set(indexer.groupIds),
  );
  const [tags, setTags] = useState(indexer.tags.join(", "));
  const [secretInputs, setSecretInputs] = useState<Record<string, string>>({});
  const [clearSecrets, setClearSecrets] = useState<Set<string>>(
    () => new Set(),
  );
  const [status, setStatus] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [loginRevision, setLoginRevision] = useState(0);
  const configured = new Set(indexer.configuredSecrets);
  const [failed, setFailed] = useState(false);
  const publicFields = indexer.configurationSchema.fields.filter(field => !field.secret);
  const siteFields = publicFields.filter(field => /url|host|site|base/i.test(field.name));
  const credentialFields = publicFields.filter(field => !siteFields.includes(field) && /user|login|email|account/i.test(field.name));
  const searchFields = publicFields.filter(field => !siteFields.includes(field) && !credentialFields.includes(field));
  function renderField(field: ConfigurationField) {
    return <PublicField key={field.name} field={field} value={values[field.name]} onChange={setValue} />;
  }

  function setValue(field: ConfigurationField, value: ConfigurationValue) {
    setValues((current) => ({ ...current, [field.name]: value }));
  }

  function toggleClear(name: string, clear: boolean) {
    setClearSecrets((current) => {
      const next = new Set(current);
      if (clear) {
        next.add(name);
      } else {
        next.delete(name);
      }
      return next;
    });
  }

  function toggleGroup(id: string, included: boolean) {
    setGroupIds((current) => {
      const next = new Set(current);
      if (included) next.add(id);
      else next.delete(id);
      return next;
    });
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (saving) return;
    let configurationSaved = false;
    setSaving(true);
    setStatus(null);
    setFailed(false);
    const secrets: Record<string, string | null> = {};
    for (const field of indexer.configurationSchema.fields) {
      if (!field.secret) {
        continue;
      }
      if (clearSecrets.has(field.name)) {
        secrets[field.name] = null;
      } else {
        const input = secretInputs[field.name];
        if (input) {
          secrets[field.name] = input;
        }
      }
    }
    try {
      // Apply sequentially so a failed organization update cannot race a configuration write.
      const configuration = await saveIndexerConfiguration(token, indexer.id, {
        userAgent: userAgent.trim() || null, flaresolverrInstanceId: connectorId || null, values, secrets,
      });
      configurationSaved = true;
      onSaved(configuration);
      setSecretInputs({});
      setClearSecrets(new Set());
      const organization = await saveIndexerOrganization(token, indexer.id, {
        displayName: displayName.trim(), enabled, proxyId: proxyId || null,
        groupIds: Array.from(groupIds).sort(),
        tags: tags.split(",").map(tag => tag.trim()).filter(Boolean),
      });
      setLoginRevision(revision => revision + 1);
      onSaved({
        ...configuration,
        displayName: organization.displayName,
        enabled: organization.enabled,
        proxyId: organization.proxyId,
        groupIds: organization.groupIds,
        tags: organization.tags,
      });
      setSecretInputs({});
      setClearSecrets(new Set());
      setStatus("Configuration saved. Secret values remain redacted.");
    } catch (error) {
      setFailed(true);
      if (error instanceof ApiError && error.status === 401) {
        onUnauthorized();
        return;
      }
      setStatus(
        `${configurationSaved ? t("Configuration saved; organization update failed. Retry Save changes.") + " " : ""}${error instanceof Error ? error.message : t("configuration save failed")}`,
      );
    } finally {
      setSaving(false);
    }
  }

  return (
    <form className="indexer-card" onSubmit={submit}>
      <header>
        <div>
          <p className="eyebrow">{t("Indexer configuration")}</p>
          <h3>{displayName}</h3>
          <p>{indexer.definitionId}</p>
        </div>
        <div className="editor-header-actions">
          <div className="card-badges">
            <span className={`privacy-pill ${indexer.privacy}`}>
              {t(indexer.privacy)}
            </span>
            <span
              className={
                indexer.catalogAvailable && enabled
                  ? "status-pill healthy"
                  : "status-pill"
              }
            >
              {t(!indexer.catalogAvailable
                ? "unavailable (retired from catalog)"
                : enabled
                  ? "enabled"
                  : "disabled")}
            </span>
          </div>
          <button
            aria-label={t("Close indexer configuration")}
            className="il-action"
            onClick={onClose}
            type="button"
          >
            ×
          </button>
        </div>
      </header>
      <IndexerCapabilitiesPanel indexer={indexer} />
      {indexer.configurationSchema.flaresolverrRecommended ? (
        <p className="recommendation">{t("This definition recommends FlareSolverr challenge handling.")}{" "}</p>
      ) : null}
      <fieldset className="il-editor-section" disabled={saving}><legend>{t("Overview")}</legend><div className="field-grid">
        <label className="wide-field">{t("Display name")}{" "}<input
            onChange={(event) => setDisplayName(event.target.value)}
            required
            value={displayName}
          />
        </label>
        <label className="checkbox-field wide-field">
          <input
            checked={enabled}
            onChange={(event) => setEnabled(event.target.checked)}
            type="checkbox"
          />{t("Enabled for tests and search")}{" "}</label>
      </div></fieldset>
      {siteFields.length > 0 && <fieldset className="il-editor-section" disabled={saving}><legend>{t("Site settings")}</legend><div className="field-grid">{siteFields.map(renderField)}</div></fieldset>}
      {(credentialFields.length > 0 || indexer.configurationSchema.fields.some(field => field.secret)) && <fieldset className="il-editor-section" disabled={saving}>
        <legend>{t("Credentials")}</legend><div className="field-grid">
          {credentialFields.map(renderField)}
          {indexer.configurationSchema.fields.filter(field => field.secret).map(field => <SecretField key={field.name} field={field}
            clear={clearSecrets.has(field.name)} configured={configured.has(field.name)} value={secretInputs[field.name] ?? ""}
            onClear={toggleClear} onValue={(name, value) => setSecretInputs(current => ({ ...current, [name]: value }))} />)}
        </div></fieldset>}
      {searchFields.length > 0 && <fieldset className="il-editor-section" disabled={saving}><legend>{t("Search settings")}</legend><div className="field-grid">{searchFields.map(renderField)}</div></fieldset>}
      <fieldset className="il-editor-section" disabled={saving}><legend>{t("Advanced routing and tags")}</legend><div className="field-grid">
        <label className="wide-field">{t("Tags (comma separated)")}{" "}<input
            onChange={(event) => setTags(event.target.value)}
            placeholder="anime, private"
            value={tags}
          />
        </label>
        <fieldset className="group-membership wide-field">
          <legend>{t("Groups")}</legend>
          <div>
            {groups.length === 0 ? (
              <small>{t("No groups configured.")}</small>
            ) : (
              groups.map((group) => (
                <label className="checkbox-field" key={group.id}>
                  <input
                    checked={groupIds.has(group.id)}
                    onChange={(event) =>
                      toggleGroup(group.id, event.target.checked)
                    }
                    type="checkbox"
                  />
                  {group.name}
                </label>
              ))
            )}
          </div>
        </fieldset>
        <label className="wide-field">{t("Outbound proxy")}{" "}<select
            onChange={(event) => setProxyId(event.target.value)}
            value={proxyId}
          >
            <option value="">{t("Inherit group or global route")}</option>
            {proxies
              .filter((proxy) => proxy.enabled || proxy.id === proxyId)
              .map((proxy) => (
                <option key={proxy.id} value={proxy.id}>
                  {proxy.name} ({proxy.kind})
                </option>
              ))}
          </select>
        </label>
        <label className="wide-field">{t("User-Agent override")}{" "}<input
            onChange={(event) => setUserAgent(event.target.value)}
            placeholder={t("Inherit the IndexLoom default")}
            value={userAgent}
          />
        </label>
        <label className="wide-field">{t("Browser challenge route")}{" "}<select
            onChange={(event) => setConnectorId(event.target.value)}
            value={connectorId}
          >
            <option value="">{t("No FlareSolverr")}</option>
            {connectors
              .filter((connector) => connector.enabled || connector.id === connectorId)
              .map((connector) => (
                <option key={connector.id} value={connector.id}>
                  {connector.name}
                </option>
              ))}
          </select>
        </label>
      </div></fieldset>
      {indexer.capabilities.authentication && <TrackerLoginPanel
        indexerId={indexer.id} token={token} revision={loginRevision}
        onUnauthorized={onUnauthorized} onComplete={() => setStatus("Tracker login completed.")}
      />}
      <div className="form-actions">
        <button className="il-action il-primary" disabled={saving} type="submit">
          {t(saving ? "Saving changes…" : "Save changes")}
        </button>
        <button className="il-action" type="button" disabled={saving} onClick={onClose}>{t("Cancel")}</button>
        {status ? (
          <p className="form-status" role={failed ? "alert" : "status"}>
            {t(status)}
          </p>
        ) : null}
      </div>
    </form>
  );
}

function PublicField({
  field,
  value,
  onChange,
}: {
  field: ConfigurationField;
  value: ConfigurationValue | undefined;
  onChange: (field: ConfigurationField, value: ConfigurationValue) => void;
}) {
  if (field.kind === "checkbox") {
    return (
      <label className="checkbox-field wide-field">
        <input
          checked={value === true}
          onChange={(event) => onChange(field, event.target.checked)}
          type="checkbox"
        />
        {t(categoryLabel(field.label))}
      </label>
    );
  }
  if (field.kind === "select") {
    return (
      <label>
        {t(categoryLabel(field.label))}
        <select
          onChange={(event) => onChange(field, event.target.value)}
          value={value === undefined ? "" : String(value)}
        >
          {field.options.map((option) => (
            <option key={option.value} value={option.value}>
              {t(categoryLabel(option.label))}
            </option>
          ))}
        </select>
      </label>
    );
  }
  return (
    <label>
      {t(categoryLabel(field.label))}
      <input
        onChange={(event) => onChange(field, event.target.value)}
        value={value === undefined ? "" : String(value)}
      />
    </label>
  );
}

function SecretField({
  field,
  configured,
  clear,
  value,
  onValue,
  onClear,
}: {
  field: ConfigurationField;
  configured: boolean;
  clear: boolean;
  value: string;
  onValue: (name: string, value: string) => void;
  onClear: (name: string, clear: boolean) => void;
}) {
  return (
    <div className="secret-field wide-field">
      <label>
        <span>
          {t(categoryLabel(field.label))}
          <small
            className={configured ? "secret-state configured" : "secret-state"}
          >
            {t(configured ? "configured" : "not configured")}
          </small>
        </span>
        <input
          autoComplete="new-password"
          disabled={clear}
          onChange={(event) => onValue(field.name, event.target.value)}
          placeholder={
            configured
              ? t("Leave blank to keep current value")
              : t("Enter secret value")
          }
          type="password"
          value={value}
        />
      </label>
      {configured ? (
        <label className="checkbox-field clear-secret">
          <input
            checked={clear}
            onChange={(event) => onClear(field.name, event.target.checked)}
            type="checkbox"
          />{t("Clear saved value")}{" "}</label>
      ) : null}
    </div>
  );
}

function initialValues(indexer: Indexer): Record<string, ConfigurationValue> {
  const values = { ...indexer.values };
  for (const field of indexer.configurationSchema.fields) {
    if (field.secret) {
      delete values[field.name];
      continue;
    }
    if (values[field.name] !== undefined) {
      continue;
    }
    if (field.defaultValue !== undefined) {
      values[field.name] = field.defaultValue;
    } else if (field.kind === "checkbox") {
      values[field.name] = false;
    } else if (field.kind === "select" && field.options[0]) {
      values[field.name] = field.options[0].value;
    }
  }
  return values;
}

/** Only upstream-supplied categories/modes are shown; local tags stay separate. */
function IndexerCapabilitiesPanel({ indexer }: { indexer: Indexer }) {
  const caps = indexer.capabilities;
  return (
    <details className="capability-disclosure" open>
      <summary>
        <span>{t("Definition details")}</span>
        <span className="muted">
          {caps.source === "jackett" ? "Jackett" : "Prowlarr"} · {languageLabel(caps.language)}
          {" · "}{formatNumber(caps.categoryMappings.length)} {t("mappings")}
        </span>
      </summary>
      <div className="capability-details">
        {caps.description ? <p>{caps.description}</p> : null}
        <dl className="capability-facts">
          <div><dt>{t("Source")}</dt><dd>{caps.source}</dd></div>
          <div><dt>{t("Language")}</dt><dd>{languageLabel(caps.language)}</dd></div>
          <div><dt>{t("Search paths")}</dt><dd>{caps.searchPaths}</dd></div>
          <div><dt>{t("Alternative URLs")}</dt><dd>{caps.alternateUrls}</dd></div>
          <div><dt>{t("Authentication")}</dt><dd>{t(caps.authentication ? "Defined" : "None declared")}</dd></div>
          <div><dt>{t("CAPTCHA")}</dt><dd>{t(caps.captcha ? "Declared" : "Not declared")}</dd></div>
          <div><dt>{t("Download handler")}</dt><dd>{t(caps.downloadHandler ? "Declared" : "None declared")}</dd></div>
          <div><dt>{t("Genre query")}</dt><dd>{t(caps.genreSearch ? "Available in definition" : "Not declared")}</dd></div>
        </dl>
        <h4>{t("Search modes")}</h4>
        {caps.searchModes.length ? (
          <div className="capability-chip-list">
            {caps.searchModes.map((mode) => (
              <span className="capability-chip" key={mode.name} title={mode.parameters.join(", ")}>
                {searchModeLabel(mode.name)} <small>{mode.parameters.join(" · ")}</small>
              </span>
            ))}
          </div>
        ) : <p className="muted">{t("No search modes declared.")}</p>}
        <h4>{t("Categories")} ({formatNumber(caps.categoryMappings.length)})</h4>
        {caps.categoryMappings.length ? (
          <div className="capability-table">
            <table>
              <thead><tr><th>{t("Tracker ID")}</th><th>{t("Category")}</th><th>{t("Description")}</th></tr></thead>
              <tbody>{caps.categoryMappings.map((category, index) => (
                <tr key={category.id + ":" + index}>
                  <td><code>{category.id}</code></td>
                  <td>{categoryLabel(category.category)}{category.default ? ` (${t("default")})` : ""}</td>
                  <td>{category.description ? categoryLabel(category.description) : "—"}</td>
                </tr>
              ))}</tbody>
            </table>
          </div>
        ) : <p className="muted">{t("No explicit category mappings; some definitions use category maps instead.")}</p>}
        <p className="muted">{t("These are declared capabilities, not a guarantee that every feature is currently supported by IndexLoom's Cardigann executor.")}</p>
      </div>
    </details>
  );
}


