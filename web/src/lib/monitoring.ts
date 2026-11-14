/** Stage D API adapters. Fields not allowlisted are never rendered, including queries/credentials. */
export type IndexerStatus = "enabled" | "disabled" | "circuit-open" | "half-open" | "unknown";
export type IndexerStats = {
  id: string; name: string; status: IndexerStatus;
  requests: number | null; failures: number | null; authErrors: number | null;
  rssOperations: number | null; downloads: number | null; meanResponseMs: number | null;
  failureStreak: number | null; backoffSeconds: number | null; lastEvent: string;
};
export type AuditEvent = { id: string; indexerId: string; indexerName: string; kind: string; outcome: string; timestamp: string };
export type EventPage = { items: AuditEvent[]; total: number | null; hasMore: boolean; nextBefore: number | null };
export type Health = { id: string; status: "healthy" | "degraded" | "unconfigured" | "unknown" };
type Obj = Record<string, unknown>;
const object = (v: unknown): Obj => v !== null && typeof v === "object" && !Array.isArray(v) ? v as Obj : {};
const str = (v: unknown): string => typeof v === "string" ? v : "";
const num = (v: unknown): number | null => typeof v === "number" && Number.isFinite(v) && v >= 0 ? v : null;
const iso = (v: unknown): string => {
  if (typeof v === "number" && Number.isFinite(v)) return new Date(v * 1000).toISOString();
  return str(v);
};
function rows(value: unknown, keys: string[]): unknown[] {
  if (Array.isArray(value)) return value;
  const data = object(value);
  for (const key of keys) if (Array.isArray(data[key])) return data[key] as unknown[];
  throw new Error("Unexpected monitoring response");
}
export function normalizeIndexers(value: unknown): IndexerStats[] {
  return rows(value, ["indexers", "items", "data"]).map((raw) => {
    const o = object(raw), s = { ...o, ...object(o.stats) }, circuit = object(o.circuitBreaker ?? o.circuit_breaker);
    const id = str(o.id ?? o.indexerId ?? o.indexer_id);
    if (!id) throw new Error("Unexpected monitoring response");
    const now = Date.now() / 1000;
    const until = num(o.openUntil ?? o.open_until);
    const probe = num(o.probeUntil ?? o.probe_until);
    const explicit = str(o.status ?? circuit.state).replaceAll("_", "-");
    const status: IndexerStatus = o.enabled === false ? "disabled"
      : until !== null && until > now ? "circuit-open"
      : probe !== null && probe > now ? "half-open"
      : explicit === "open" ? "circuit-open"
      : ["enabled", "disabled", "circuit-open", "half-open"].includes(explicit) ? explicit as IndexerStatus
      : o.enabled === true ? "enabled" : "unknown";
    return {
      id, name: str(o.displayName ?? o.display_name ?? o.name ?? o.indexerName) || id, status,
      requests: num(s.searches ?? s.requests ?? s.requestCount ?? s.request_count),
      failures: num(s.failedQueries ?? s.failed_queries ?? s.failures ?? s.failureCount ?? s.failure_count),
      authErrors: num(s.authErrors ?? s.auth_errors), rssOperations: num(s.rss ?? s.rssOperations ?? s.rss_operations),
      downloads: num(s.downloads), meanResponseMs: num(s.averageResponseMs ?? s.meanResponseMs ?? s.mean_response_ms ?? s.average_response_ms),
      failureStreak: num(o.consecutiveFailures ?? o.consecutive_failures ?? o.failureStreak ?? circuit.failureStreak ?? circuit.failure_streak),
      backoffSeconds: until !== null ? Math.max(0, Math.ceil(until - now)) : num(o.backoffSeconds ?? circuit.backoffSeconds),
      lastEvent: iso(o.lastActivity ?? o.last_activity ?? o.lastEventAt ?? o.last_event_at ?? o.lastEvent),
    };
  });
}
export function normalizeEvents(value: unknown, _limit: number): EventPage {
  const root = object(value);
  const items = rows(value, ["events", "items", "data"]).map((raw) => {
    const e = object(raw);
    return {
      id: typeof e.id === "number" ? String(e.id) : str(e.id),
      indexerId: str(e.indexerId ?? e.indexer_id),
      indexerName: str(e.indexerName ?? e.indexer_name),
      kind: str(e.category ?? e.kind),
      outcome: str(e.status ?? e.outcome),
      timestamp: iso(e.timestamp ?? e.createdAt ?? e.created_at),
    };
  });
  const cursor = num(root.nextBefore ?? root.next_before);
  return { items, total: num(root.total ?? root.totalCount), hasMore: cursor !== null, nextBefore: cursor };
}
export const dependencies = ["database", "indexers", "proxies", "connectedApps", "mail", "rssScheduler", "systemTime"] as const;
const ok = (v: unknown): Health["status"] => {
  const status = typeof v === "string" ? v : str(object(v).status);
  return status === "ok" || status === "healthy" || status === "scheduled" ? "healthy"
    : ["error", "unavailable", "open", "half-open", "degraded", "stale", "overdue", "failure"].includes(status) ? "degraded"
    : status === "unconfigured" || status === "disabled" ? "unconfigured" : "unknown";
};
function aggregate(v: unknown): Health["status"] {
  if (!Array.isArray(v) || !v.length) return "unconfigured";
  const enabled = v.map(object).filter((row) => row.enabled !== false);
  if (!enabled.length) return "unconfigured";
  const statuses = enabled.map((entry) => ok(entry));
  if (statuses.includes("degraded")) return "degraded";
  if (statuses.includes("unknown")) return "unknown";
  return statuses.includes("healthy") ? "healthy" : "unconfigured";
}
export function normalizeHealth(value: unknown): Health[] {
  const root = object(value);
  if ("components" in root) {
    const components = object(root.components);
    const calculated: Record<string, Health["status"]> = {
      database: ok(root.database),
      indexers: aggregate(root.indexers),
      proxies: aggregate(components.proxies),
      connectedApps: aggregate(components.applications),
      mail: ok(components.mail),
      rssScheduler: aggregate(components.rssScheduler),
      systemTime: ok(components.clockSkew),
    };
    return dependencies.map((id) => ({ id, status: calculated[id] ?? "unknown" }));
  }
  const source = root.dependencies ?? root.checks ?? root.health ?? value;
  const aliases: Record<string, string> = { connectedApps: "connected_apps", rssScheduler: "rss_scheduler", systemTime: "system_time" };
  return dependencies.map((id) => {
    const entry = Array.isArray(source)
      ? source.find((v) => [id, aliases[id]].includes(str(object(v).id ?? object(v).name)))
      : object(source)[id] ?? object(source)[aliases[id] ?? id];
    return { id, status: ok(entry) };
  });
}
export async function monitoringRequest(path: string, token: string, signal: AbortSignal, method = "GET"): Promise<unknown> {
  const response = await fetch(path, { method, signal: AbortSignal.any([signal, AbortSignal.timeout(20_000)]), cache: "no-store", headers: { Accept: "application/json", ...(token && token !== "disabled" ? { Authorization: "Bearer " + token } : {}) } });
  if (!response.ok) throw new Error("Monitoring request failed (HTTP " + response.status + ")");
  if (response.status === 204) return null;
  try { return await response.json() as unknown; } catch { throw new Error("Unexpected monitoring response"); }
}
export function eventQuery(filters: { indexerId: string; kind: string; outcome: string; before: string; after: string }, beforeId: number | null, limit: number): string {
  const params = new URLSearchParams({ limit: String(limit) });
  if (beforeId !== null) params.set("before", String(beforeId));
  if (filters.indexerId) params.set("indexerId", filters.indexerId);
  if (filters.kind) params.set("category", filters.kind);
  if (filters.outcome) params.set("status", filters.outcome);
  if (filters.after) params.set("since", String(Math.floor(new Date(filters.after).getTime() / 1000)));
  if (filters.before) params.set("until", String(Math.floor(new Date(filters.before).getTime() / 1000)));
  return "/api/v1/monitoring/events?" + params;
}
