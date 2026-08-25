export type Health = {
  status: "ok" | "degraded";
  version: string;
  database: "ok" | "unavailable";
};

export async function fetchHealth(signal: AbortSignal): Promise<Health> {
  const response = await fetch("/api/v1/health", {
    headers: { Accept: "application/json" },
    signal,
  });

  if (!response.ok) {
    throw new Error(`health endpoint returned ${response.status}`);
  }

  return (await response.json()) as Health;
}
