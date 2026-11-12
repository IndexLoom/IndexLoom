import { createContext, type ReactNode, useCallback, useContext, useMemo, useState } from "react";
import { ruMessages } from "./messages.ru";
import { ruExtra } from "./messages.ru.extra";
import { ruDynamic } from "./messages.ru.dynamic";
import { canonicalGenres, categoryRu, genreRu, newznabCategoryNames } from "./taxonomy";
import { extraLocaleMessages } from "./messages.additional";
export { canonicalGenres } from "./taxonomy";

export const LOCALE_NAMES = {
  en: "English", ru: "Русский", de: "Deutsch", es: "Español", fr: "Français",
  pt: "Português", it: "Italiano", uk: "Українська", pl: "Polski",
  tr: "Türkçe", ja: "日本語", zh: "中文", ko: "한국어",
} as const;
export type Locale = keyof typeof LOCALE_NAMES;
export const SUPPORTED_LOCALES: readonly Locale[] = Object.keys(LOCALE_NAMES) as Locale[];
export const LOCALE_AREAS = ["navigation", "search", "indexers", "settings", "taxonomy"] as const;
export type LocaleArea = typeof LOCALE_AREAS[number];
type Overrides = Partial<Record<LocaleArea, Locale>>;
type CustomMessages = Partial<Record<Locale, Record<string, string>>>;
const AREA_KEY = "indexloom.locale.areas.v1";
const CUSTOM_KEY = "indexloom.locale.custom.v1";
function isLocale(value: unknown): value is Locale {
  return typeof value === "string" && Object.prototype.hasOwnProperty.call(LOCALE_NAMES, value);
}
function storedAreas(): Overrides {
  try {
    const raw: unknown = JSON.parse(window.localStorage.getItem(AREA_KEY) ?? "{}");
    if (typeof raw !== "object" || !raw || Array.isArray(raw)) return {};
    const result: Overrides = {};
    for (const area of LOCALE_AREAS) {
      const value = (raw as Record<string, unknown>)[area];
      if (isLocale(value)) result[area] = value;
    }
    return result;
  } catch { return {}; }
}
function storedCustom(): CustomMessages {
  try {
    const raw: unknown = JSON.parse(window.localStorage.getItem(CUSTOM_KEY) ?? "{}");
    if (!raw || typeof raw !== "object" || Array.isArray(raw)) return {};
    const result: CustomMessages = {};
    for (const locale of SUPPORTED_LOCALES) {
      const entries = (raw as Record<string, unknown>)[locale];
      if (!entries || typeof entries !== "object" || Array.isArray(entries)) continue;
      result[locale] = {};
      for (const [key, val] of Object.entries(entries).slice(0, 200)) {
        if (key.length <= 250 && typeof val === "string" && val.length <= 1000) result[locale]![key] = val;
      }
    }
    return result;
  } catch { return {}; }
}
let activeAreas: Overrides = typeof window === "undefined" ? {} : storedAreas();
let activeCustom: CustomMessages = typeof window === "undefined" ? {} : storedCustom();
export function getLocaleFor(area?: LocaleArea): Locale {
  return (area && activeAreas[area]) || activeLocale;
}
export function translate(message: string, variables?: Record<string, string | number>, area?: LocaleArea): string {
  return t(message, variables, area);
}
const STORAGE_KEY = "indexloom.locale.v1";
const fallbackMessages: Readonly<Record<string, string>> = { ...ruMessages, ...ruExtra, ...ruDynamic };
const englishToGenre = new Map<string, string>();

for (const genre of Object.keys(genreRu)) {
  englishToGenre.set(genre.toLocaleLowerCase("en"), genre);
  // Some Russian labels are shared by multiple English synonyms. Preserve the
  // first, stable canonical spelling unless a preset genre overrides it.
  const localized = genreRu[genre];
  if (localized) {
    const normalized = localized.toLocaleLowerCase("ru");
    if (!englishToGenre.has(normalized)) englishToGenre.set(normalized, genre);
  }
}
for (const genre of canonicalGenres) {
  const label = genreRu[genre];
  if (label) englishToGenre.set(label.toLocaleLowerCase("ru"), genre);
}

function initialLocale(): Locale {
  if (typeof window === "undefined") return "en";
  try {
    const saved = window.localStorage.getItem(STORAGE_KEY);
    if (isLocale(saved)) return saved;
  } catch {
    // Private browsing may disable persistent storage.
  }
  const languages = navigator.languages?.length ? navigator.languages : [navigator.language];
  for (const name of languages) {
    const code = name.toLowerCase().split("-")[0];
    if (isLocale(code)) return code;
  }
  return "en";
}

let activeLocale: Locale = initialLocale();

export function getLocale(): Locale {
  return activeLocale;
}

/** Translate a stable, English UI message. Unknown text remains unchanged. */
export function t(message: string, variables?: Record<string, string | number>, area?: LocaleArea): string {
  const normalized = message.replace(/\s+/g, " ").trim();
  const locale = getLocaleFor(area);
  let translated = activeCustom[locale]?.[normalized]
    ?? (locale === "ru" ? fallbackMessages[normalized] : extraLocaleMessages[locale]?.[normalized])
    ?? message;
  if (variables) {
    for (const [name, value] of Object.entries(variables)) {
      translated = translated.replaceAll(`{{${name}}}`, String(value));
    }
  }
  return translated;
}

export function formatNumber(value: number): string {
  return new Intl.NumberFormat(activeLocale).format(value);
}

export function formatDateTime(value: string | number | null | undefined): string {
  if (value == null || value === "") return "—";
  const date = new Date(typeof value === "number" ? value : value);
  if (Number.isNaN(date.valueOf())) return String(value);
  return new Intl.DateTimeFormat(activeLocale, {
    year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit",
  }).format(date);
}

/** Localize common Cardigann hierarchy labels while retaining original ID values. */
export function categoryLabel(original: string | number | null | undefined): string {
  if (original == null || original === "") return "—";
  const input = String(original);
  const locale = getLocaleFor("taxonomy");
  const known = newznabCategoryNames[input] ?? input;
  if (locale === "en") return known;
  return known.split(/(\s*[/|>→]\s*)/).map((part) => {
    const key = part.trim();
    if (!key || /^[\\/|>→]+$/.test(key)) return part;
    if (locale === "ru") return activeCustom.ru?.[key] ?? categoryRu[key] ?? genreRu[key] ?? part;
    return activeCustom[locale]?.[key] ?? extraLocaleMessages[locale]?.[key] ?? part;
  }).join("");
}

/** Genre values accepted from third-party source definitions, displayed locally. */
export function genreLabel(input: string): string {
  const locale = getLocaleFor("taxonomy");
  return activeCustom[locale]?.[input] ?? (locale === "ru" ? genreRu[input] : extraLocaleMessages[locale]?.[input]) ?? input;
}

/** Convert localized suggestions back to stable English values before Cardigann search. */
export function canonicalGenre(input: string): string {
  const trimmed = input.trim();
  for (const [locale, entries] of Object.entries(activeCustom)) {
    if (!isLocale(locale)) continue;
    for (const [original, translated] of Object.entries(entries ?? {})) {
      if (translated.toLocaleLowerCase() === trimmed.toLocaleLowerCase() && (original in genreRu || canonicalGenres.includes(original))) return original;
    }
  }
  return englishToGenre.get(trimmed.toLocaleLowerCase()) ?? trimmed;
}

/** Indexer metadata languages, independent of the UI's two supported locales. */
export function languageLabel(code: string): string {
  if (!code) return "—";
  try {
    const display = new Intl.DisplayNames([getLocaleFor("taxonomy")], {
      type: "language", fallback: "code",
    });
    const key = code.replace("_", "-");
    return display.of(key) ?? code;
  } catch {
    return code;
  }
}

/** Names of Cardigann search modes are UI strings, never translated in API payloads. */
export function searchModeLabel(mode: string): string {
  const locale = getLocaleFor("taxonomy");
  if (locale !== "ru") return t(mode, undefined, "taxonomy");
  return ({
    search: "Поиск", "movie-search": "Поиск фильмов", "tv-search": "Поиск сериалов",
    "music-search": "Поиск музыки", "book-search": "Поиск книг",
    "audio-search": "Поиск аудио",
  } as Record<string, string>)[mode] ?? t(mode);
}

type LocaleContextValue = {
  locale: Locale;
  setLocale: (next: Locale) => void;
  areas: Overrides;
  setAreaLocale: (area: LocaleArea, locale: Locale | null) => void;
  customMessages: CustomMessages;
  setCustomMessage: (locale: Locale, key: string, value: string) => void;
};
const LocaleContext = createContext<LocaleContextValue | null>(null);

export function LocaleProvider({ children }: { children: ReactNode }) {
  const [locale, updateLocale] = useState<Locale>(() => activeLocale);
  const [areas, updateAreas] = useState<Overrides>(() => activeAreas);
  const [customMessages, updateCustom] = useState<CustomMessages>(() => activeCustom);
  const setLocale = useCallback((next: Locale) => {
    if (!isLocale(next)) return;
    activeLocale = next;
    updateLocale(next);
    try {
      window.localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // In-memory locale still works in private mode.
    }
    document.documentElement.lang = next;
    document.title = next === "ru" ? "IndexLoom — Поиск и индексаторы" : "IndexLoom — Search and Indexers";
  }, []);
  // Set HTML semantics synchronously for the initial mount.
  if (typeof document !== "undefined") {
    document.documentElement.lang = locale;
    document.documentElement.dir = "ltr";
  }
  const setAreaLocale = useCallback((area: LocaleArea, next: Locale | null) => {
    const updated = { ...activeAreas };
    if (next === null) delete updated[area];
    else if (isLocale(next)) updated[area] = next;
    else return;
    activeAreas = updated;
    updateAreas(updated);
    try { window.localStorage.setItem(AREA_KEY, JSON.stringify(updated)); } catch { /* private mode */ }
  }, []);
  const setCustomMessage = useCallback((next: Locale, key: string, translated: string) => {
    const original = key.replace(/\s+/g, " ").trim();
    if (!isLocale(next) || !original || original.length > 250 || translated.length > 1000) return;
    const entries = { ...activeCustom[next] };
    if (translated) entries[original] = translated;
    else delete entries[original];
    if (Object.keys(entries).length > 200) return;
    const updated = { ...activeCustom, [next]: entries };
    activeCustom = updated;
    updateCustom(updated);
    try { window.localStorage.setItem(CUSTOM_KEY, JSON.stringify(updated)); } catch { /* private mode */ }
  }, []);
  const value = useMemo(() => ({ locale, setLocale, areas, setAreaLocale, customMessages, setCustomMessage }),
    [locale, setLocale, areas, setAreaLocale, customMessages, setCustomMessage]);
  return <LocaleContext.Provider value={value}>{children}</LocaleContext.Provider>;
}

export function useLocale(): LocaleContextValue {
  const value = useContext(LocaleContext);
  if (!value) throw new Error("LocaleProvider missing");
  return value;
}

export function LanguageSwitcher({ compact = false }: { compact?: boolean }) {
  const { locale, setLocale } = useLocale();
  return (
    <label className={compact ? "language-picker compact" : "language-picker"}>
      {!compact ? <span>{t("Interface language")}</span> : null}
      <select
        aria-label={t("Interface language")}
        value={locale}
        onChange={(event) => setLocale(event.target.value as Locale)}
      >
        {SUPPORTED_LOCALES.map((lang) => <option key={lang} value={lang}>{LOCALE_NAMES[lang]}</option>)}
      </select>
    </label>
  );
}

/** Pick independent language for each UI area; default inherits the global locale.
 * Custom phrases are local to this browser and never sent to indexers. */
export function SelectiveLocalizationEditor() {
  const { locale, areas, setAreaLocale, customMessages, setCustomMessage } = useLocale();
  const [target, setTarget] = useState<Locale>(locale);
  const [source, setSource] = useState("Search");
  const [translation, setTranslation] = useState("");
  const examples = ["Search", "Indexers", "History", "Settings", "Genre", "Category", "Search across indexers", "Advanced search filters", "Movie", "Movies", "Anime", "Fantasy"];
  return <div className="setting-heading">
    <h2>{t("Selective localization")}</h2>
    <p>{t("Choose an independent language for each workspace. Original tracker titles and query IDs are never modified.")}</p>
    <div className="locale-area-grid">
      {LOCALE_AREAS.map((area) => <label key={area}>
        <span>{t(({ navigation: "Navigation", search: "Search", indexers: "Indexers", settings: "Settings", taxonomy: "Categories and genres" })[area])}</span>
        <select aria-label={area + " language"} value={areas[area] ?? ""} onChange={(event) => setAreaLocale(area, event.target.value ? event.target.value as Locale : null)}>
          <option value="">{t("Use interface language")}</option>
          {SUPPORTED_LOCALES.map((lang) => <option key={lang} value={lang}>{LOCALE_NAMES[lang]}</option>)}
        </select>
      </label>)}
    </div>
    <h3>{t("Custom translations")}</h3>
    <p>{t("Override a single original English phrase in the chosen language. Empty translation removes the override.")}</p>
    <div className="locale-area-grid">
      <label>{t("Target language")}
        <select aria-label="Target translation language" value={target} onChange={(event) => { setTarget(event.target.value as Locale); setTranslation(""); }}>
          {SUPPORTED_LOCALES.map((lang) => <option key={lang} value={lang}>{LOCALE_NAMES[lang]}</option>)}
        </select>
      </label>
      <label>{t("Original phrase")}
        <input list="locale-phrase-examples" maxLength={250} value={source} onChange={(event) => { setSource(event.target.value); setTranslation(""); }} />
        <datalist id="locale-phrase-examples">{examples.map((item) => <option key={item} value={item} />)}</datalist>
      </label>
      <label>{t("Translation")}
        <input maxLength={1000} value={translation} placeholder={customMessages[target]?.[source] ?? ""} onChange={(event) => setTranslation(event.target.value)} />
      </label>
    </div>
    <button className="quiet-button" type="button" onClick={() => { setCustomMessage(target, source, translation); setTranslation(""); }}>
      {t("Save translation")}
    </button>
    <div className="muted">{t("Saved phrases")}: {Object.keys(customMessages[target] ?? {}).length}</div>
  </div>;
}
