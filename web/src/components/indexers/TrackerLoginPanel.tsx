import { useEffect, useState } from "react";
import { ApiError } from "../../lib/api";
import { trackerChallengeImage, trackerLogin, type TrackerChallenge } from "../../lib/tracker-login";
import { translate } from "../../lib/i18n";
const t = (message: string) => translate(message, undefined, "indexers");

export function TrackerLoginPanel({ indexerId, token, revision, onUnauthorized, onComplete }: {
  indexerId: string;
  token: string;
  revision: number;
  onUnauthorized: () => void;
  onComplete: () => void;
}) {
  const [challenge, setChallenge] = useState<TrackerChallenge | null>(null);
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const [image, setImage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setChallenge(null); setAnswers({}); setImage(null); setError(null); setMessage(null);
  }, [indexerId, token, revision]);

  useEffect(() => {
    if (!challenge) return;
    const timer = window.setTimeout(() => {
      setChallenge(null); setAnswers({}); setImage(null);
      setError("Tracker challenge expired. Start tracker login again.");
    }, challenge.expiresInSeconds * 1000);
    return () => window.clearTimeout(timer);
  }, [challenge]);

  useEffect(() => {
    setImage(null);
    if (!challenge?.hasImage) return;
    const controller = new AbortController();
    let url: string | undefined;
    trackerChallengeImage(token, indexerId, challenge.id, controller.signal).then(blob => {
      if (controller.signal.aborted) return;
      url = URL.createObjectURL(blob); setImage(url);
    }).catch((cause: unknown) => {
      if (controller.signal.aborted) return;
      if (cause instanceof ApiError && cause.status === 401) onUnauthorized();
      setError(cause instanceof Error ? cause.message : "Unable to load tracker CAPTCHA. Start tracker login again.");
    });
    return () => { controller.abort(); if (url) URL.revokeObjectURL(url); };
  }, [challenge, indexerId, onUnauthorized, token]);

  async function advance() {
    setBusy(true); setError(null); setMessage(null); setAnswers({});
    try {
      const progress = await trackerLogin(token, indexerId, challenge?.id, answers);
      if (progress.status === "pending") setChallenge(progress.challenge);
      else { setChallenge(null); setMessage("Tracker login completed."); onComplete(); }
    } catch (cause) {
      setChallenge(null);
      if (cause instanceof ApiError && cause.status === 401) onUnauthorized();
      setError(cause instanceof Error ? cause.message : "Tracker login failed. Start tracker login again.");
    } finally { setBusy(false); }
  }

  return <fieldset className="tracker-login-panel" disabled={busy}>
    <legend>{t("Tracker login")}</legend>
    <p>{t("Save credentials first, then start tracker login. Codes and CAPTCHA answers are used once and are not saved.")}</p>
    <p className="muted">{t("Image and text CAPTCHA and tracker verification codes are supported. JavaScript CAPTCHA requires a browser; Cloudflare may require FlareSolverr.")}</p>
    {challenge && <div>
      {challenge.prompt && <p>{challenge.prompt}</p>}
      {challenge.hasImage && (image ? <img src={image} alt={t("Tracker CAPTCHA")} style={{ maxWidth: "100%" }} /> : <p role="status">{t("Loading tracker CAPTCHA…")}</p>)}
      <p>{t("Complete this step within five minutes. Expired or rejected answers require a new login.")}</p>
      {challenge.fields.map(field => <label className="field" key={field.name}>
        <span>{t(field.label)}</span>
        <input value={answers[field.name] ?? ""} maxLength={512} autoComplete="off" type="password"
          onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); if (!busy && challenge.fields.every(f => answers[f.name]?.trim())) void advance(); } }}
          onChange={event => setAnswers(current => ({ ...current, [field.name]: event.target.value }))} />
      </label>)}
    </div>}
    <button className="quiet-button" type="button" onClick={() => void advance()}
      disabled={busy || Boolean(challenge && (challenge.fields.some(f => !answers[f.name]?.trim()) || (challenge.hasImage && !image)))}>
      {t(busy ? "Contacting tracker…" : challenge ? "Submit tracker response" : "Start tracker login")}
    </button>
    {error && <p role="alert" className="form-status">{t(error)}</p>}
    {message && <p role="status" className="form-status">{t(message)}</p>}
  </fieldset>;
}
