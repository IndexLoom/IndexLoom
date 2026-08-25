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
  values: Record<string, ConfigurationValue>;
  configuredSecrets: string[];
  configurationSchema: {
    fields: ConfigurationField[];
    flaresolverrRecommended: boolean;
    userAgentRecommended: boolean;
  };
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

export function fetchIndexers(token: string, signal: AbortSignal): Promise<Indexer[]> {
  return authenticatedJson<Indexer[]>("/api/v1/indexers", token, { signal });
}

export function fetchFlareSolverr(
  token: string,
  signal: AbortSignal,
): Promise<FlareSolverrInstance[]> {
  return authenticatedJson<FlareSolverrInstance[]>("/api/v1/flaresolverr", token, {
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

export async function removeFlareSolverr(token: string, id: string): Promise<void> {
  const response = await fetch(`/api/v1/flaresolverr/${encodeURIComponent(id)}`, {
    cache: "no-store",
    headers: { Authorization: `Bearer ${token}` },
    method: "DELETE",
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
