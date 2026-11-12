import { createContext, type ReactNode, useCallback, useContext, useMemo, useState } from "react";
import { ruMessages } from "./messages.ru";
import { ruExtra } from "./messages.ru.extra";
import { canonicalGenres, categoryRu, genreRu, newznabCategoryNames } from "./taxonomy";
export { canonicalGenres } from "./taxonomy";

export type Locale = "en" | "ru";
export const SUPPORTED_LOCALES: readonly Locale[] = ["en", "ru"];
const STORAGE_KEY = "indexloom.locale.v1";
const fallbackMessages: Readonly<Record<string, string>> = { ...ruMessages, ...ruExtra };
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
    if (saved === "ru" || saved === "en") return saved;
  } catch {
    // Private browsing may disable persistent storage.
  }
  const languages = navigator.languages?.length ? navigator.languages : [navigator.language];
  return languages.some((name) => name.toLowerCase().startsWith("ru")) ? "ru" : "en";
}

let activeLocale: Locale = initialLocale();

export function getLocale(): Locale {
  return activeLocale;
}

/** Translate a stable, English UI message. Unknown text remains unchanged. */
export function t(message: string, variables?: Record<string, string | number>): string {
  const normalized = message.replace(/\s+/g, " ").trim();
  let translated = activeLocale === "ru" ? fallbackMessages[normalized] ?? message : message;
  if (variables) {
    for (const [name, value] of Object.entries(variables)) {
      translated = translated.replaceAll(`{{${name}}}`, String(value));
    }
  }
  return translated;
}

export function formatNumber(value: number): string {
  return new Intl.NumberFormat(activeLocale === "ru" ? "ru-RU" : "en-US").format(value);
}

export function formatDateTime(value: string | number | null | undefined): string {
  if (value == null || value === "") return "—";
  const date = new Date(typeof value === "number" ? value : value);
  if (Number.isNaN(date.valueOf())) return String(value);
  return new Intl.DateTimeFormat(activeLocale === "ru" ? "ru-RU" : "en-US", {
    year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit",
  }).format(date);
}

/** Localize common Cardigann hierarchy labels while retaining original ID values. */
export function categoryLabel(original: string | number | null | undefined): string {
  if (original == null || original === "") return "—";
  const input = String(original);
  if (activeLocale === "en") return input;
  const known = newznabCategoryNames[input] ?? input;
  if (Object.prototype.hasOwnProperty.call(categoryRu, known)) return categoryRu[known] ?? known;
  return known.split(/(\s*[/|>→]\s*)/).map((part) => {
    const key = part.trim();
    const exact = categoryRu[key] ?? genreRu[key];
    return exact ?? part;
  }).join("");
}

/** Genre values accepted from third-party source definitions, displayed locally. */
export function genreLabel(input: string): string {
  if (activeLocale === "en") return input;
  return genreRu[input] ?? input;
}

/** Convert localized suggestions back to stable English values before Cardigann search. */
export function canonicalGenre(input: string): string {
  const trimmed = input.trim();
  return englishToGenre.get(trimmed.toLocaleLowerCase("ru")) ?? trimmed;
}

/** Indexer metadata languages, independent of the UI's two supported locales. */
export function languageLabel(code: string): string {
  if (!code) return "—";
  try {
    const display = new Intl.DisplayNames(activeLocale === "ru" ? ["ru"] : ["en"], {
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
  if (activeLocale === "en") return mode;
  return ({
    search: "Поиск", "movie-search": "Поиск фильмов", "tv-search": "Поиск сериалов",
    "music-search": "Поиск музыки", "book-search": "Поиск книг",
    "audio-search": "Поиск аудио",
  } as Record<string, string>)[mode] ?? t(mode);
}

type LocaleContextValue = {
  locale: Locale;
  setLocale: (next: Locale) => void;
};
const LocaleContext = createContext<LocaleContextValue | null>(null);

export function LocaleProvider({ children }: { children: ReactNode }) {
  const [locale, updateLocale] = useState<Locale>(() => activeLocale);
  const setLocale = useCallback((next: Locale) => {
    if (next !== "en" && next !== "ru") return;
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
  const value = useMemo(() => ({ locale, setLocale }), [locale, setLocale]);
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
        <option value="ru">Русский</option>
        <option value="en">English</option>
      </select>
    </label>
  );
}
