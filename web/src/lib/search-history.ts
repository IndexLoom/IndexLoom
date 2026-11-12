/** Account-scoped completed searches. Never store result links or credentials.
 * Local storage is not an access-control boundary against other OS/browser users.
 * Incompatible legacy unscoped history is deleted rather than exposed to a different account. */
export type HistoryEntry = {
  id: string;
  query: string;
  completedAt: string;
  resultCount: number;
  indexerCount: number;
  language: string | null;
};
const LEGACY_KEY = "indexloom.search-history.v1";
const KEY_PREFIX = "indexloom.search-history.v2.";
const MAX_ENTRIES = 100;
const MAX_SERIALIZED = 65_536;

function storageKey(scope: string): string | null {
  if (!/^(?:user:[a-zA-Z0-9-]{1,128}|operator|anonymous)$/.test(scope)) return null;
  return KEY_PREFIX + scope;
}
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
export function readSearchHistory(scope: string): HistoryEntry[] {
  const key = storageKey(scope);
  if (!key) return [];
  try {
    localStorage.removeItem(LEGACY_KEY);
    const raw = localStorage.getItem(key);
    if (!raw || raw.length > MAX_SERIALIZED) return [];
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter(valid).slice(0, MAX_ENTRIES) : [];
  } catch { return []; }
}
export function recordSearch(scope: string, entry: Omit<HistoryEntry, "id" | "completedAt">): void {
  const key = storageKey(scope);
  if (!key) return;
  const next = {
    ...entry, completedAt: new Date().toISOString(),
    id: crypto.randomUUID(),
  };
  if (!valid(next)) return;
  const entries = [next, ...readSearchHistory(scope)].slice(0, MAX_ENTRIES);
  try { localStorage.setItem(key, JSON.stringify(entries)); } catch { /* Storage disabled. */ }
}
export function deleteSearchHistory(scope: string, id?: string): HistoryEntry[] {
  const key = storageKey(scope);
  if (!key) return [];
  const entries = id ? readSearchHistory(scope).filter((entry) => entry.id !== id) : [];
  try {
    if (entries.length) localStorage.setItem(key, JSON.stringify(entries));
    else localStorage.removeItem(key);
  } catch { /* Storage disabled. */ }
  return entries;
}
