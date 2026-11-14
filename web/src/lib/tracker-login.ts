import { ApiError } from "./api";

export type TrackerChallenge = {
  id: string;
  expiresInSeconds: number;
  fields: { name: string; label: string; kind: string }[];
  prompt: string | null;
  hasImage: boolean;
};
export type TrackerLoginProgress =
  | { status: "complete" }
  | { status: "pending"; challenge: TrackerChallenge };

const base = (indexer: string) => `/api/v1/indexers/${encodeURIComponent(indexer)}/login-challenge`;
async function checked(response: Response): Promise<Response> {
  if (!response.ok) {
    const body = await response.json().catch(() => ({})) as { message?: string };
    throw new ApiError(response.status, body.message ?? `API returned ${response.status}`);
  }
  return response;
}
export async function trackerLogin(token: string, indexer: string, challenge?: string, answers?: Record<string, string>): Promise<TrackerLoginProgress> {
  const response = await checked(await fetch(`${base(indexer)}${challenge ? `/${encodeURIComponent(challenge)}` : ""}`, {
    method: "POST", credentials: "omit", cache: "no-store", redirect: "error",
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json", Accept: "application/json" },
    body: JSON.stringify(challenge ? { answers } : {}),
  }));
  return response.json() as Promise<TrackerLoginProgress>;
}
export async function trackerChallengeImage(token: string, indexer: string, challenge: string, signal: AbortSignal): Promise<Blob> {
  const response = await checked(await fetch(`${base(indexer)}/${encodeURIComponent(challenge)}/image`, {
    credentials: "omit", cache: "no-store", redirect: "error", signal,
    headers: { Authorization: `Bearer ${token}`, Accept: "image/png,image/jpeg,image/gif,image/webp" },
  }));
  return response.blob();
}
