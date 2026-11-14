import { useEffect, useState } from "react";
import { AdminTokenGate, useAdminSession } from "./AdminSession";
import { translate } from "../lib/i18n";
import { useSystemRevision } from "../lib/events";
const t = (message: string) => translate(message, undefined, "settings");
type Release = { tag_name: string; html_url: string; published_at: string | null; prerelease: boolean; body: string | null; assets: { id: number; name: string; size: number; browser_download_url: string }[] };
type History = { currentVersion: string; repository: string; checkedAt: number; releases: Release[]; verification: string; installPolicy: string };
function githubLink(value: string) { try { const url = new URL(value); return url.protocol === "https:" && url.hostname === "github.com" && !url.username && !url.password ? url.href : undefined; } catch { return undefined; } }
function platform(name: string) {
  if (/\.ipa$/i.test(name)) return t("iOS (signed distribution required)");
  if (/android|\.apk$/i.test(name)) return "Android";
  if (/windows|\.exe$|\.msi$/i.test(name)) return "Windows";
  if (/macos|darwin|\.dmg$/i.test(name)) return "macOS";
  if (/linux|\.deb$|\.rpm$|appimage/i.test(name)) return "Linux";
  return t("Checksums / other platforms");
}
export function UpdatesSettings() {
  const { token, acceptToken } = useAdminSession();
  const revision = useSystemRevision(["settings"]);
  const [data, setData] = useState<History | null>(null); const [error, setError] = useState("");
  const [preview, setPreview] = useState(false); const [check, setCheck] = useState(0); const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!token) return;
    const controller = new AbortController(); setBusy(true); setError("");
    void fetch(`/api/v1/system/updates/history?prerelease=${preview}`, { signal: controller.signal, cache: "no-store", headers: { Authorization: `Bearer ${token}` } })
      .then(async response => { if (!response.ok) throw new Error(t("Could not check updates. Try again in five minutes.")); setData(await response.json() as History); })
      .catch(error => { if (!controller.signal.aborted) setError(String(error)); })
      .finally(() => { if (!controller.signal.aborted) setBusy(false); });
    return () => controller.abort();
  }, [token, preview, check, revision]);
  if (!token) return <AdminTokenGate onSubmit={acceptToken} />;
  return <div className="appearance-content">
    <div className="setting-heading"><h2>{t("Updates")}</h2><p>{t("Check official releases and download packages for your platform. Installation is manual.")}</p></div>
    <label><input type="checkbox" checked={preview} onChange={event => setPreview(event.target.checked)} />{t("Include prereleases")}</label>{" "}
    <button type="button" disabled={busy} onClick={() => setCheck(value => value + 1)}>{t("Check for updates")}</button>
    <p role="status">{busy ? t("Checking updates…") : data ? `${t("Current version")}: ${data.currentVersion} · ${t("Latest release")}: ${data.releases[0]?.tag_name ?? t("No releases published")} · ${new Date(data.checkedAt * 1000).toLocaleString()}` : ""}</p>
    {error ? <p role="alert">{error}</p> : null}
    <p>{t("Checks are cached for five minutes. SHA256 checksums verify file integrity, not publisher identity. Use the server update download command to verify a package before installing.")}</p>
    <p>{t("Docker updates use your container update policy. iOS requires signed platform distribution. IndexLoom never overwrites the running application.")}</p>
    <h3>{t("Version history")}</h3>
    {data?.releases.map(release => <details key={release.tag_name}>
      <summary>{release.tag_name} · {release.published_at ? new Date(release.published_at).toLocaleDateString() : t("Date unavailable")} {release.prerelease ? t("Prerelease") : ""}</summary>
      <a href={githubLink(release.html_url)} target="_blank" rel="noreferrer">{t("View release on GitHub")}</a>
      <pre style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>{release.body}</pre>
      <ul>{release.assets.map(asset => <li key={asset.id}><a href={githubLink(asset.browser_download_url)} target="_blank" rel="noreferrer">{asset.name}</a> · {platform(asset.name)} · {asset.size.toLocaleString()} {t("bytes")} · {t("Not verified locally")}</li>)}</ul>
    </details>)}
  </div>;
}
