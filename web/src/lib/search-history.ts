/** Browser-private search history: queries and counts only, never credentials or result URLs.
 * Stored per browser, bounded to 100 entries, and removable without server involvement. */
export type HistoryEntry = {
  id: string;
  query: string;
  completedAt: string;
  resultCount: number;
  indexerCount: number;
  language: string | null;
};
const KEY = "indexloom.search-history.v1";
const MAX_ENTRIES = 100;
const MAX_SERIALIZED = 65_536;
function valid(entry: unknown): entry is HistoryEntry {
  if (!entry || typeof entry !== "object" || Array.isArray(entry)) return false;
  const value = entry as Record<string, unknown>;
  return typeof value.id === "string" && value.id.length <= 80
    && typeof value.query === "string" && value.query.length > 0 && value.query.length <= 500
    && typeof value.completedAt === "string" && !Number.isNaN(Date.parse(value.completedAt))
    && typeof value.resultCount === "number" && Number.isSafeInteger(value.resultCount) && value.resultCount >= 0
    && typeof value.indexerCount === "number" && Number.isSafeInteger(value.indexerCount) && value.indexerCount >= 0
    && (value.language === null || typeof value.language === "string" && value.language.length <= 35);
}
export function readSearchHistory(): HistoryEntry[] {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw || raw.length > MAX_SERIALIZED) return [];
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter(valid).slice(0, MAX_ENTRIES) : [];
  } catch { return []; }
}
export function recordSearch(entry: Omit<HistoryEntry, "id" | "completedAt">) {
  const next = {
    ...entry, completedAt: new Date().toISOString(),
    id: crypto.randomUUID(),
  };
  if (!valid(next)) return;
  const entries = [next, ...readSearchHistory()].slice(0, MAX_ENTRIES);
  try { localStorage.setItem(KEY, JSON.stringify(entries)); } catch { /* Storage disabled. */ }
}
export function deleteSearchHistory(id?: string): HistoryEntry[] {
  const entries = id ? readSearchHistory().filter((entry) => entry.id !== id) : [];
  try {
    if (entries.length) localStorage.setItem(KEY, JSON.stringify(entries));
    else localStorage.removeItem(KEY);
  } catch { /* Storage disabled. */ }
  return entries;
}
