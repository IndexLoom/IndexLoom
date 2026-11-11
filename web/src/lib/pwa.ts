export function registerPwa(): void {
  if (!("serviceWorker" in navigator)) return;
  window.addEventListener("load", () => {
    void navigator.serviceWorker.register("/sw.js", { scope: "/" }).catch((error: unknown) => {
      console.warn("IndexLoom offline shell registration failed", error);
    });
  });
}
