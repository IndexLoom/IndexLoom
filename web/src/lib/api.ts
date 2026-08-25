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

export type Indexer = {
  id: string;
  definitionId: string;
  displayName: string;
  enabled: boolean;
  privacy: "public" | "semi-private" | "private";
  userAgent: string | null;
  flaresolverrInstanceId: string | null;
  proxyId: string | null;
  groupIds: string[];
  tags: string[];
  lastTest: IndexerTestResult | null;
  values: Record<string, ConfigurationValue>;
  configuredSecrets: string[];
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

export type SearchResult = {
  id: string;
  indexerId: string;
  title: string;
  link: string;
  details: string | null;
  category: string | null;
  size: number | null;
  seeders: number | null;
  leechers: number | null;
  publishedAt: string | null;
};

export type SearchIndexerResults = {
  indexerId: string;
  indexerName: string;
  responseTimeMs: number;
  results: SearchResult[];
  error: string | null;
};

export type SearchResponse = {
  query: string;
  indexers: SearchIndexerResults[];
  resultCount: number;
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
): Promise<SearchResponse> {
  return authenticatedJson<SearchResponse>("/api/v1/search", token, {
    body: JSON.stringify({ query, selection, limitPerIndexer }),
    method: "POST",
  });
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
