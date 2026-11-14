import { translate } from "../../lib/i18n";
const t = (message: string, variables?: Record<string, string | number>) => translate(message, variables, "indexers");
import { useState } from "react";
import type { Indexer, IndexerGroup } from "../../lib/api";
import { managementRequest } from "../../lib/management-api";
export function BulkIndexerPanel({ token, indexers, groups, onChanged }: { token: string; indexers: Indexer[]; groups: IndexerGroup[]; onChanged: () => Promise<void> }) {
  const [selected, setSelected] = useState<string[]>([]); const [filter, setFilter] = useState("");
  const [tags, setTags] = useState(""); const [groupIds, setGroupIds] = useState<string[]>([]); const [mode, setMode] = useState("append");
  const [busy, setBusy] = useState(false); const [error, setError] = useState(""); const [message, setMessage] = useState("");
  const filtered = indexers.filter(i => `${i.displayName} ${i.id} ${i.tags.join(" ")}`.toLowerCase().includes(filter.toLowerCase()));
  async function apply(patch: Record<string, unknown>) {
    setBusy(true); setError(""); setMessage("");
    try {
      const result = await (await managementRequest(token, "indexers/bulk", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ ids: selected, ...patch }) })).json() as { count: number };
      setSelected([]); setMessage(t(patch.delete ? "Deleted {count} indexers." : "Updated {count} indexers.", { count: result.count }));
      await onChanged();
    } catch (e) { setError(e instanceof Error ? e.message : "Bulk update failed"); } finally { setBusy(false); }
  }
  return <section><h2>{t("Bulk management")}</h2><p>{t("Select up to 500 indexers. Narrow the filter to select all when more than 500 match. Every operation is applied atomically.")}</p>
    <label>{t("Filter indexers")}<input value={filter} onChange={e => setFilter(e.target.value)} /></label>
    <button disabled={busy || filtered.length > 500} onClick={() => setSelected(filtered.map(i => i.id))}>{t("Select all filtered ({count})", { count: filtered.length })}</button>
    <button disabled={busy} onClick={() => setSelected([])}>{t("Clear selection")}</button>
    <fieldset disabled={busy}><legend>{t("{count} selected", { count: selected.length })}</legend>
      <button disabled={!selected.length} onClick={() => void apply({ enabled: true })}>{t("Enable")}</button>
      <button disabled={!selected.length} onClick={() => void apply({ enabled: false })}>{t("Disable")}</button>
      <p>{t("Use up to 50 unique tags, each at most 50 bytes. Replace with an empty list to clear tags.")}</p>
      <label>{t("Tags (comma separated)")}<input value={tags} onChange={e => setTags(e.target.value)} /></label>
      <label>{t("Tag operation")}<select value={mode} onChange={e => setMode(e.target.value)}>{["append", "remove", "replace"].map(m => <option key={m}>{m}</option>)}</select></label>
      <button disabled={!selected.length} onClick={() => void apply({ tags: { mode, values: tags.split(",").map(s => s.trim()).filter(Boolean) } })}>{t("Apply tags")}</button>
      <p>{t("Select one or more groups. Leave the selection empty to remove all groups.")}</p>
      <label>{t("Group assignment")}<select multiple value={groupIds} onChange={e => setGroupIds(Array.from(e.target.selectedOptions, option => option.value))}>{groups.map(g => <option key={g.id} value={g.id}>{g.name}</option>)}</select></label>
      <button disabled={!selected.length} onClick={() => void apply({ groupIds })}>{t("Replace group assignments")}</button>
      <button disabled={!selected.length} onClick={() => { if (window.confirm(t("Delete {count} indexers and their saved credentials?", { count: selected.length }))) void apply({ delete: true }); }}>{t("Delete selected")}</button>
    </fieldset>
    <button disabled={busy} onClick={() => void onChanged().catch(e => setError(String(e)))}>{t("Refresh indexers")}</button>
    <p role="status">{message}</p><p role="alert">{error}</p>
    <div style={{ maxHeight: "24rem", overflow: "auto" }}>{filtered.map(i => <label key={i.id} style={{ display: "block" }}><input type="checkbox" disabled={busy || (!selected.includes(i.id) && selected.length >= 500)} checked={selected.includes(i.id)} onChange={e => setSelected(current => e.target.checked ? [...current, i.id] : current.filter(id => id !== i.id))} />{i.displayName} ({t(i.enabled ? "enabled" : "disabled")})</label>)}</div>
  </section>;
}
