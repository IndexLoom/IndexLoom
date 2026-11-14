import { SseParser } from "./events-parser";
import { useEffect, useState } from "react";
export type SystemChange = { id: string; kind: string; timestamp: number };
export function useEventChannel(token: string | null) {
  const [status, setStatus] = useState("Disconnected");
  useEffect(() => {
    if (!token) { setStatus("Disconnected"); return; }
    const controller = new AbortController(); let lastId = ""; let retry = 1000;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function connect() {
      setStatus(lastId ? "Reconnecting…" : "Connecting…");
      try {
        const response = await fetch("/api/v1/events/stream", { headers: { Authorization: `Bearer ${token}`, Accept: "text/event-stream", ...(lastId ? { "Last-Event-ID": lastId } : {}) }, signal: controller.signal, cache: "no-store" });
        if (response.status === 401 || response.status === 403) { setStatus("Authentication required"); return; }
        if (!response.ok || !response.body || !response.headers.get("content-type")?.startsWith("text/event-stream")) throw new Error("Event stream unavailable");
        setStatus("Live updates connected"); retry = 1000;
        const reader = response.body.getReader(); const decoder = new TextDecoder();
        const parser = new SseParser((type, id, data) => {
          if (type === "reset") { lastId = ""; window.dispatchEvent(new CustomEvent("indexloom:change", { detail: { kind: "reset" } })); }
          if (type === "change") {
            const change = JSON.parse(data) as SystemChange;
            if (typeof change.kind !== "string" || typeof change.id !== "string") throw new Error("Invalid event");
            if (id) lastId = id;
            window.dispatchEvent(new CustomEvent("indexloom:change", { detail: change }));
          }
        });
        try { for (;;) { const chunk = await reader.read(); if (chunk.done) break; parser.push(decoder.decode(chunk.value, { stream: true })); } }
        finally { await reader.cancel(); reader.releaseLock(); }
      } catch { /* Retry transient failures; credentials never enter URLs or storage. */ }
      if (!controller.signal.aborted) { setStatus("Reconnecting…"); timer = setTimeout(() => { void connect(); }, retry + Math.random() * 500); retry = Math.min(retry * 2, 30000); }
    }
    void connect();
    return () => { controller.abort(); clearTimeout(timer); };
  }, [token]);
  return status;
}
/** Coalesce event bursts before refreshing a workspace. */
export function useSystemRevision(kinds: readonly string[]) {
  const [revision, setRevision] = useState(0); const filter = kinds.join(",");
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const listener = (event: Event) => {
      const change = (event as CustomEvent<SystemChange>).detail;
      if (change.kind !== "reset" && !filter.split(",").includes(change.kind)) return;
      if (!timer) timer = setTimeout(() => { timer = undefined; setRevision(v => v + 1); }, 300);
    };
    window.addEventListener("indexloom:change", listener);
    return () => { clearTimeout(timer); window.removeEventListener("indexloom:change", listener); };
  }, [filter]);
  return revision;
}
