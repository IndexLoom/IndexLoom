"use strict";
const $ = (id) => document.getElementById(id);
const feedback = (message) => { $("feedback").textContent = message; };
const invoke = (name, args = {}) => {
  if (!window.__TAURI__?.core?.invoke) throw new Error("Native bridge unavailable");
  return window.__TAURI__.core.invoke(name, args);
};
try {
  const previous = localStorage.getItem("indexloom.native.remote-url.v1");
  if (previous) $("remote-url").value = previous;
} catch { /* Storage may be blocked. */ }
void invoke("platform_info").then((platform) => {
  $("platform").textContent = platform;
}).catch(() => {});
let localEndpoint = null;
$("start-local").addEventListener("click", async () => {
  $("start-local").disabled = true;
  feedback("Starting local indexer engine…");
  try {
    localEndpoint = await invoke("start_autonomous");
    $("operator-token").textContent = localEndpoint.operatorToken;
    $("local-ready").hidden = false;
    feedback("Local IndexLoom started on this device.");
  } catch (error) {
    feedback(String(error));
  } finally {
    $("start-local").disabled = false;
  }
});
$("copy-token").addEventListener("click", async () => {
  if (!localEndpoint) return;
  try {
    await navigator.clipboard.writeText(localEndpoint.operatorToken);
    feedback("Token copied.");
  } catch {
    feedback("Clipboard unavailable; copy the token shown above.");
  }
});
$("open-local").addEventListener("click", () => {
  if (localEndpoint) window.location.assign(localEndpoint.url);
});
$("remote-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const button = $("remote-form").querySelector("button");
  button.disabled = true;
  feedback("Validating HTTPS server…");
  try {
    const url = await invoke("validate_external", { url: $("remote-url").value });
    try { localStorage.setItem("indexloom.native.remote-url.v1", url); } catch {}
    window.location.assign(url);
  } catch (error) {
    feedback(String(error));
    button.disabled = false;
  }
});
