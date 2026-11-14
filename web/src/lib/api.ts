export type CatalogStatus = {
  totalDefinitions: number;
  availableDefinitions: number;
  retiredDefinitions: number;
  latestReleaseAppliedAt: string | null;
  latestReleaseFingerprint: string | null;
};

export type Health = {
  status: "ok" | "degraded";
  version: string;
  database: "ok" | "unavailable";
};

export type ConfigurationValue = string | number | boolean;

export type ConfigurationOption = {
  value: string;
  label: string;
};

export type ConfigurationField = {
  name: string;
  label: string;
  kind: "text" | "password" | "checkbox" | "select";
  secret: boolean;
  defaultValue?: ConfigurationValue;
  options: ConfigurationOption[];
};

export type IndexerCapabilities = {
  description: string;
  language: string;
  source: "jackett" | "prowlarr-indexers";
  searchModes: Array<{ name: string; parameters: string[] }>;
  categoryMappings: Array<{ id: string; category: string; description: string | null; default: boolean }>;
  categories: string[];
  genreSearch: boolean;
  rawSearch: boolean;
  authentication: boolean;
  captcha: boolean;
  downloadHandler: boolean;
  searchPaths: number;
  alternateUrls: number;
};

export type Indexer = {
  id: string;
  definitionId: string;
  displayName: string;
  enabled: boolean;
  catalogAvailable: boolean;
  privacy: "public" | "semi-private" | "private";
  userAgent: string | null;
  flaresolverrInstanceId: string | null;
  proxyId: string | null;
  groupIds: string[];
  tags: string[];
  lastTest: IndexerTestResult | null;
  values: Record<string, ConfigurationValue>;
  configuredSecrets: string[];
  capabilities: IndexerCapabilities;
  configurationSchema: {
    fields: ConfigurationField[];
    flaresolverrRecommended: boolean;
    userAgentRecommended: boolean;
  };
};

export type ProxyKind = "http" | "socks4" | "socks5";

export type OutboundProxy = {
  id: string;
  name: string;
  kind: ProxyKind;
  host: string;
  port: number;
  enabled: boolean;
  username: string | null;
  passwordConfigured: boolean;
  createdAt: string;
  updatedAt: string;
};

export type OutboundProxyUpdate = {
  name: string;
  kind: ProxyKind;
  host: string;
  port: number;
  enabled: boolean;
  username: string | null;
  password?: string;
  clearPassword: boolean;
};

export type IndexerGroup = {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  priority: number;
  proxyId: string | null;
  memberCount: number;
  createdAt: string;
  updatedAt: string;
};

export type IndexerGroupUpdate = {
  name: string;
  description: string;
  enabled: boolean;
  priority: number;
  proxyId: string | null;
};

export type RoutingSettings = {
  globalProxyId: string | null;
};

export type IndexerOrganizationUpdate = {
  displayName: string;
  enabled: boolean;
  proxyId: string | null;
  groupIds: string[];
  tags: string[];
};

export type SearchFilters = {
  /** Restrict configured indexers by metadata language, not the language of titles. */
  languages?: string[];
  mode?: string;
  category?: string;
  genre?: string;
  year?: number;
  season?: number;
  episode?: number;
  imdbId?: string;
  tmdbId?: string;
  tvdbId?: string; tvmazeId?: string; traktId?: string; doubanId?: string;
  tvrageId?: string; artist?: string; album?: string; author?: string;
  title?: string; publisher?: string; label?: string; track?: string;
  group?: string; episodeCode?: string;
  minSize?: number; maxSize?: number; minSeeders?: number; maxAgeDays?: number;
};

export type IndexerSelection = {
  all: boolean;
  indexerIds: string[];
  groupIds: string[];
};

export type IndexerTestResult = {
  indexerId?: string;
  indexerName?: string;
  status?: "success" | "failed";
  success?: boolean;
  responseTimeMs: number | null;
  message: string;
  testedAt?: string;
};

export type IndexerTestJob = {
  id: string;
  status: "running" | "completed";
  total: number;
  completed: number;
  results: Array<
    Required<
      Pick<
        IndexerTestResult,
        "indexerId" | "indexerName" | "success" | "message"
      >
    > & {
      responseTimeMs: number;
    }
  >;
  createdAt: string;
  finishedAt: string | null;
};

export type ReleaseProtocol = "torrent" | "usenet";
export type ReleaseAttributes = {
  files: number | null; grabs: number | null; poster: string | null;
  genres: string[]; languages: string[]; subtitles: string[];
  artist: string | null; album: string | null; author: string | null;
  publisher: string | null; group: string | null; uploader: string | null;
  downloadVolumeFactor: number | null; uploadVolumeFactor: number | null;
  minimumRatio: number | null; minimumSeedTime: number | null;
  imdbId: string | null; tmdbId: string | null; tvdbId: string | null;
  tvmazeId: string | null; traktId: string | null; doubanId: string | null;
  tvrageId: string | null;
};

export type SearchResult = {
  id: string;
  indexerInstanceId: string;
  definitionId: string;
  sourceId: string | null;
  sourceGuid: string | null;
  infoHash: string | null;
  title: string;
  detailsUrl: string | null;
  downloadUrl: string | null;
  magnetUrl: string | null;
  category: string | null;
  size: number | null;
  seeders: number | null;
  leechers: number | null;
  publishedAt: string | null;
  protocol: ReleaseProtocol;
  attributes: ReleaseAttributes;
};

export type SearchIndexerResults = {
  indexerInstanceId: string;
  definitionId: string | null;
  indexerName: string;
  responseTimeMs: number;
  results: SearchResult[];
  error: string | null;
};

export type UsenetSource = {
  id:string; name:string; endpoint:string; enabled:boolean; keyConfigured:boolean;
};
export type UsenetSourceUpdate = {
  name:string; endpoint:string; enabled:boolean; apiKey?:string;
};
export function fetchUsenetSources(token:string):Promise<UsenetSource[]>{
  return authenticatedJson<UsenetSource[]>("/api/v1/usenet/indexers",token);
}
export function saveUsenetSource(token:string,id:string,update:UsenetSourceUpdate):Promise<UsenetSource>{
  return authenticatedJson<UsenetSource>("/api/v1/usenet/indexers/"+encodeURIComponent(id),token,{
    method:"PUT",body:JSON.stringify(update)
  });
}
export function deleteUsenetSource(token:string,id:string):Promise<{deleted:boolean}>{
  return authenticatedJson<{deleted:boolean}>("/api/v1/usenet/indexers/"+encodeURIComponent(id),token,{method:"DELETE"});
}

export type SortField = "publishedAt" | "size" | "seeders" | "leechers" | "grabs" | "files" | "indexer" | "title";
export type ResultOptions = {
  deduplicate: boolean;
  sortBy: SortField;
  descending: boolean;
  minSize?: number; maxSize?: number; minSeeders?: number; maxAgeDays?: number;
  protocol?: ReleaseProtocol; categories: string[];
  includeIndexers: string[]; excludeIndexers: string[];
  titleContains?: string; indexerPriorities: Record<string,number>;
};
export type SearchPreset = {
  id: string; name: string; createdAt: string; updatedAt: string;
  document: { filters: SearchFilters; resultOptions: ResultOptions; selection: IndexerSelection };
};
export const DEFAULT_RESULT_OPTIONS:ResultOptions = {
  deduplicate:true,sortBy:"publishedAt",descending:true,categories:[],
  includeIndexers:[],excludeIndexers:[],indexerPriorities:{},
};
export function fetchSearchPresets(token:string):Promise<SearchPreset[]>{
  return authenticatedJson<SearchPreset[]>("/api/v1/search/presets",token);
}
export function saveSearchPreset(token:string,id:string,name:string,document:SearchPreset["document"]):Promise<{saved:boolean}>{
  return authenticatedJson<{saved:boolean}>("/api/v1/search/presets/"+encodeURIComponent(id),token,{
    method:"PUT",body:JSON.stringify({name,document}),
  });
}
export function deleteSearchPreset(token:string,id:string):Promise<{deleted:boolean}>{
  return authenticatedJson<{deleted:boolean}>("/api/v1/search/presets/"+encodeURIComponent(id),token,{method:"DELETE"});
}

export type SearchResponse = {
  query: string;
  indexers: SearchIndexerResults[];
  resultCount: number;
  uniqueResults: SearchResult[];
  duplicateCount: number;
  filteredCount: number;
};

export type SearchStreamEvent =
  | { type: "started"; query: string; total: number }
  | { type: "indexer"; indexer: SearchIndexerResults }
  | {
      type: "finished";
      query: string;
      completed: number;
      total: number;
      resultCount: number;
      uniqueResults: SearchResult[];
      duplicateCount: number;
      filteredCount: number;
    };

export type FlareSolverrInstance = {
  id: string;
  name: string;
  endpoint: string;
  enabled: boolean;
  timeoutSeconds: number;
  createdAt: string;
  updatedAt: string;
};

export type IndexerConfigurationUpdate = {
  userAgent: string | null;
  flaresolverrInstanceId: string | null;
  values: Record<string, ConfigurationValue>;
  secrets: Record<string, string | null>;
};

export type FlareSolverrUpdate = {
  name: string;
  endpoint: string;
  enabled: boolean;
  timeoutSeconds: number;
};

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

export async function fetchHealth(signal: AbortSignal): Promise<Health> {
  const response = await fetch("/api/v1/health", {
    cache: "no-store",
    headers: { Accept: "application/json" },
    signal,
  });
  return readJson<Health>(response);
}

export function fetchCatalogStatus(
  token: string,
  signal?: AbortSignal,
): Promise<CatalogStatus> {
  return authenticatedJson<CatalogStatus>("/api/v1/catalog/status", token, {
    signal,
  });
}

export function fetchIndexers(
  token: string,
  signal: AbortSignal,
): Promise<Indexer[]> {
  return authenticatedJson<Indexer[]>("/api/v1/indexers", token, { signal });
}

export function fetchFlareSolverr(
  token: string,
  signal: AbortSignal,
): Promise<FlareSolverrInstance[]> {
  return authenticatedJson<FlareSolverrInstance[]>(
    "/api/v1/flaresolverr",
    token,
    {
      signal,
    },
  );
}

export function fetchProxies(
  token: string,
  signal?: AbortSignal,
): Promise<OutboundProxy[]> {
  return authenticatedJson<OutboundProxy[]>("/api/v1/proxies", token, {
    signal,
  });
}

export function fetchGroups(
  token: string,
  signal?: AbortSignal,
): Promise<IndexerGroup[]> {
  return authenticatedJson<IndexerGroup[]>("/api/v1/groups", token, { signal });
}

export function fetchRouting(
  token: string,
  signal?: AbortSignal,
): Promise<RoutingSettings> {
  return authenticatedJson<RoutingSettings>("/api/v1/routing", token, {
    signal,
  });
}

export function saveIndexerConfiguration(
  token: string,
  id: string,
  update: IndexerConfigurationUpdate,
): Promise<Indexer> {
  return authenticatedJson<Indexer>(
    `/api/v1/indexers/${encodeURIComponent(id)}/configuration`,
    token,
    {
      body: JSON.stringify(update),
      method: "PUT",
    },
  );
}

export function saveFlareSolverr(
  token: string,
  id: string,
  update: FlareSolverrUpdate,
): Promise<FlareSolverrInstance> {
  return authenticatedJson<FlareSolverrInstance>(
    `/api/v1/flaresolverr/${encodeURIComponent(id)}`,
    token,
    {
      body: JSON.stringify(update),
      method: "PUT",
    },
  );
}

export function saveIndexerOrganization(
  token: string,
  id: string,
  update: IndexerOrganizationUpdate,
): Promise<Indexer> {
  return authenticatedJson<Indexer>(
    `/api/v1/indexers/${encodeURIComponent(id)}/organization`,
    token,
    { body: JSON.stringify(update), method: "PUT" },
  );
}

export function saveProxy(
  token: string,
  id: string,
  update: OutboundProxyUpdate,
): Promise<OutboundProxy> {
  return authenticatedJson<OutboundProxy>(
    `/api/v1/proxies/${encodeURIComponent(id)}`,
    token,
    {
      body: JSON.stringify(update),
      method: "PUT",
    },
  );
}

export async function removeProxy(token: string, id: string): Promise<void> {
  return authenticatedEmpty(
    `/api/v1/proxies/${encodeURIComponent(id)}`,
    token,
    "DELETE",
  );
}

export function saveGroup(
  token: string,
  id: string,
  update: IndexerGroupUpdate,
): Promise<IndexerGroup> {
  return authenticatedJson<IndexerGroup>(
    `/api/v1/groups/${encodeURIComponent(id)}`,
    token,
    {
      body: JSON.stringify(update),
      method: "PUT",
    },
  );
}

export async function removeGroup(token: string, id: string): Promise<void> {
  return authenticatedEmpty(
    `/api/v1/groups/${encodeURIComponent(id)}`,
    token,
    "DELETE",
  );
}

export function saveRouting(
  token: string,
  update: RoutingSettings,
): Promise<RoutingSettings> {
  return authenticatedJson<RoutingSettings>("/api/v1/routing", token, {
    body: JSON.stringify(update),
    method: "PUT",
  });
}

export function startIndexerTests(
  token: string,
  selection: IndexerSelection,
): Promise<IndexerTestJob> {
  return authenticatedJson<IndexerTestJob>("/api/v1/indexer-tests", token, {
    body: JSON.stringify(selection),
    method: "POST",
  });
}

export function fetchIndexerTestJob(
  token: string,
  id: string,
): Promise<IndexerTestJob> {
  return authenticatedJson<IndexerTestJob>(
    `/api/v1/indexer-tests/${encodeURIComponent(id)}`,
    token,
  );
}

export function searchIndexers(
  token: string,
  query: string,
  selection: IndexerSelection,
  limitPerIndexer = 50,
  filters: SearchFilters = {},
  resultOptions: ResultOptions = DEFAULT_RESULT_OPTIONS,
): Promise<SearchResponse> {
  return authenticatedJson<SearchResponse>("/api/v1/search", token, {
    body: JSON.stringify({ query, selection, limitPerIndexer, filters: withoutLanguages(filters), languages: filters.languages ?? [], resultOptions }),
    method: "POST",
  });
}

export async function searchIndexersStream(
  token: string,
  query: string,
  selection: IndexerSelection,
  onEvent: (event: SearchStreamEvent) => void,
  signal?: AbortSignal,
  limitPerIndexer = 50,
  filters: SearchFilters = {},
  resultOptions: ResultOptions = DEFAULT_RESULT_OPTIONS,
): Promise<void> {
  const response = await fetch("/api/v1/search/stream", {
    body: JSON.stringify({ query, selection, limitPerIndexer, filters: withoutLanguages(filters), languages: filters.languages ?? [], resultOptions }),
    cache: "no-store",
    headers: {
      Accept: "application/x-ndjson",
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
    },
    method: "POST",
    signal,
  });
  if (!response.ok) {
    throw await responseError(response);
  }
  if (!response.body) {
    throw new ApiError(response.status, "search stream has no response body");
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffered = "";
  let finished = false;
  for (;;) {
    const chunk = await reader.read();
    buffered += decoder.decode(chunk.value, { stream: !chunk.done });
    let newline = buffered.indexOf("\n");
    while (newline >= 0) {
      const line = buffered.slice(0, newline).trim();
      buffered = buffered.slice(newline + 1);
      if (line) {
        const event = parseSearchStreamEvent(line);
        finished ||= event.type === "finished";
        onEvent(event);
      }
      newline = buffered.indexOf("\n");
    }
    if (chunk.done) break;
  }
  if (buffered.trim()) {
    const event = parseSearchStreamEvent(buffered.trim());
    finished ||= event.type === "finished";
    onEvent(event);
  }
  if (!finished) {
    throw new ApiError(502, "search stream ended before its final summary");
  }
}

function parseSearchStreamEvent(line: string): SearchStreamEvent {
  const event = JSON.parse(line) as Partial<SearchStreamEvent>;
  if (
    event.type !== "started" &&
    event.type !== "indexer" &&
    event.type !== "finished"
  ) {
    throw new ApiError(502, "search stream returned an unknown event");
  }
  return event as SearchStreamEvent;
}

export async function removeFlareSolverr(
  token: string,
  id: string,
): Promise<void> {
  const response = await fetch(
    `/api/v1/flaresolverr/${encodeURIComponent(id)}`,
    {
      cache: "no-store",
      headers: { Authorization: `Bearer ${token}` },
      method: "DELETE",
    },
  );
  if (!response.ok) {
    throw await responseError(response);
  }
}

async function authenticatedEmpty(
  path: string,
  token: string,
  method: string,
): Promise<void> {
  const response = await fetch(path, {
    cache: "no-store",
    headers: { Authorization: `Bearer ${token}` },
    method,
  });
  if (!response.ok) {
    throw await responseError(response);
  }
}

async function authenticatedJson<T>(
  path: string,
  token: string,
  init: RequestInit = {},
): Promise<T> {
  const headers = new Headers(init.headers);
  headers.set("Accept", "application/json");
  headers.set("Authorization", `Bearer ${token}`);
  if (init.body) {
    headers.set("Content-Type", "application/json");
  }
  const response = await fetch(path, {
    ...init,
    cache: "no-store",
    headers,
  });
  return readJson<T>(response);
}

async function readJson<T>(response: Response): Promise<T> {
  if (!response.ok) {
    throw await responseError(response);
  }
  return (await response.json()) as T;
}

async function responseError(response: Response): Promise<ApiError> {
  let message = `API returned ${response.status}`;
  try {
    const body = (await response.json()) as { message?: string };
    if (body.message) {
      message = body.message;
    }
  } catch {
    // Preserve the status-only fallback for non-JSON proxy errors.
  }
  return new ApiError(response.status, message);
}

/** Keep UI-only indexer-language restriction separate from Cardigann search parameters. */
function withoutLanguages(filters: SearchFilters): Omit<SearchFilters, "languages"> {
  const { languages: _languages, ...trackerFilters } = filters;
  return trackerFilters;
}

/** Consume a server-issued, one-time download handle with the current credential. */
export async function downloadRelease(token: string, path: string, title: string): Promise<void> {
  if (!/^\/api\/v1\/downloads\/[a-f0-9]{64}$/.test(path)) {
    throw new ApiError(400, "Invalid download handle");
  }
  const response = await fetch(path, {
    cache: "no-store",
    redirect: "error",
    headers: { Authorization: `Bearer ${token}` },
  });
  if (!response.ok) throw await responseError(response);
  if (response.headers.get("Content-Type")?.startsWith("application/json")) {
    const body = await response.json() as { magnetUrl?: string };
    if (!body.magnetUrl?.startsWith("magnet:?")) throw new ApiError(502, "Invalid magnet response");
    window.location.assign(body.magnetUrl);
    return;
  }
  if (!response.headers.get("Content-Type")?.startsWith("application/x-bittorrent")) {
    throw new ApiError(502, "Invalid torrent response");
  }
  const blob = await response.blob();
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = `${title.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").slice(0, 150) || "download"}.torrent`;
  link.rel = "noreferrer";
  document.body.append(link);
  link.click();
  link.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 30_000);
}
