import type { EventPage, IndexerStats } from "../../lib/monitoring";
import { metric, Status, t, timestamp } from "./shared";
type Filters = { indexerId: string; kind: string; outcome: string; before: string; after: string };
type Props = { filters: Filters; onFilter: (key: keyof Filters, value: string) => void; indexers: IndexerStats[]; page: EventPage | null; loading: boolean; invalidDates: boolean; pageIndex: number; onPrevious: () => void; onNext: () => void };
export function AuditTable({ filters, onFilter, indexers, page, loading, invalidDates, pageIndex, onPrevious, onNext }: Props) {
  return <><div className="monitoring-filters">
    <label>{t("Indexer")}<select value={filters.indexerId} onChange={(e) => onFilter("indexerId", e.target.value)}><option value="">{t("All")}</option>{indexers.map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}</select></label>
    <label>{t("Kind")}<select value={filters.kind} onChange={(e) => onFilter("kind", e.target.value)}><option value="">{t("All")}</option>{["search", "rss", "download", "auth", "test", "reset", "errors", "results"].map((v) => <option key={v} value={v}>{t(v)}</option>)}</select></label>
    <label>{t("Outcome")}<select value={filters.outcome} onChange={(e) => onFilter("outcome", e.target.value)}><option value="">{t("All")}</option>{["success", "error", "pending"].map((v) => <option key={v} value={v}>{t(v)}</option>)}</select></label>
    {(["after", "before"] as const).map((key) => <label key={key}>{t(key === "after" ? "After (local time)" : "Before (local time)")}<input type="datetime-local" value={filters[key]} aria-invalid={invalidDates} aria-describedby={invalidDates ? "monitoring-date-error" : undefined} onChange={(e) => onFilter(key, e.target.value)} /></label>)}
    <button type="button" className="quiet-button" onClick={() => { for (const key of Object.keys(filters) as (keyof Filters)[]) onFilter(key, ""); }}>{t("Clear filters")}</button>
  </div>{invalidDates ? <p id="monitoring-date-error" role="alert">{t("After must be earlier than Before.")}</p> : null}
  {page && !invalidDates ? <><div className="monitoring-scroll" tabIndex={0} role="region" aria-label={t("Audit activity")}><table><caption>{t("Audit activity")}</caption><thead><tr>{["Time", "Indexer", "Kind", "Outcome"].map((label) => <th scope="col" key={label}>{t(label)}</th>)}</tr></thead><tbody>{page.items.map((event, i) => <tr key={event.id + ":" + i}><td>{timestamp(event.timestamp)}</td><td>{event.indexerName || indexers.find((item) => item.id === event.indexerId)?.name || event.indexerId || "—"}</td><td>{t(event.kind) || "—"}</td><td><Status value={event.outcome || "unknown"} /></td></tr>)}</tbody></table></div>
  {page.items.length === 0 ? <p>{t("No audit events match. Clear filters or refresh after new activity.")}</p> : null}
  <div className="monitoring-pagination"><button type="button" className="quiet-button" disabled={loading || pageIndex === 0} onClick={onPrevious}>{t("Previous")}</button>
    <span>{t("Page")} {pageIndex + 1}</span>
    <button type="button" className="quiet-button" disabled={loading || !page.hasMore} onClick={onNext}>{t("Next")}</button></div></> : null}</>;
}
