import { useState } from "react";
import { formatDateTime, formatNumber, languageLabel, translate } from "../lib/i18n";
import { deleteSearchHistory, readSearchHistory } from "../lib/search-history";

const t = (phrase: string) => translate(phrase, undefined, "search");

export function HistoryWorkspace({ onReplay }: { onReplay: (query: string) => void }) {
  const [items, setItems] = useState(readSearchHistory);
  const count = items.length;
  const total = items.reduce((sum, entry) => sum + entry.resultCount, 0);
  return <section className="settings-view">
    <header className="page-heading">
      <div>
        <p className="eyebrow">{t("Search activity")}</p>
        <h1>{t("History")}</h1>
        <p className="page-description">{t("Saved in this browser only. Queries, timestamps and counts; never tracker credentials or links.")}</p>
      </div>
    </header>
    <div className="overview-metrics">
      <div className="overview-metric"><strong>{formatNumber(count)}</strong><span>{t("Completed searches")}</span></div>
      <div className="overview-metric"><strong>{formatNumber(total)}</strong><span>{t("Results found")}</span></div>
    </div>
    <div className="appearance-footer">
      <button className="quiet-button" type="button" disabled={count === 0}
        onClick={() => { if (window.confirm(t("Clear all search history?"))) setItems(deleteSearchHistory()); }}>
        {t("Clear history")}
      </button>
    </div>
    {count === 0 ? <p className="muted">{t("No completed searches yet.")}</p> : null}
    <div className="history-list">
      {items.map((entry) => <article className="history-row" key={entry.id}>
        <div className="history-main">
          <strong>{entry.query}</strong>
          <small>{formatDateTime(entry.completedAt)} · {formatNumber(entry.resultCount)} {t("results")} · {formatNumber(entry.indexerCount)} {t("indexers")}{entry.language ? " · " + languageLabel(entry.language) : ""}</small>
        </div>
        <div className="history-actions">
          <button className="quiet-button" type="button" onClick={() => onReplay(entry.query)}>{t("Search again")}</button>
          <button className="quiet-button" aria-label={t("Delete history entry")} type="button" onClick={() => setItems(deleteSearchHistory(entry.id))}>{t("Delete")}</button>
        </div>
      </article>)}
    </div>
  </section>;
}
