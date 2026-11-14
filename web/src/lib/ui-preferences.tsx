import { createContext, type ReactNode, useContext, useEffect, useMemo, useState } from "react";

export type Palette = {
  page: string; surface: string; raised: string; sunken: string;
  text: string; muted: string; border: string; accent: string;
};
export type PaletteKey = keyof Palette;
export const paletteKeys: PaletteKey[] = ["page", "surface", "raised", "sunken", "text", "muted", "border", "accent"];
export const themes = {
  obsidian: { name: "Obsidian Violet", dark: true, colors: ["#09090f", "#12111b", "#1b1928", "#08070d", "#f3efff", "#a49db5", "#302a43", "#a17aff"] },
  graphite: { name: "Carbon", dark: true, colors: ["#0b0c0f", "#14161b", "#20232a", "#08090c", "#f4f5f7", "#a0a6b0", "#343840", "#d0d4db"] },
  midnight: { name: "Midnight Blue", dark: true, colors: ["#080f1b", "#101a2b", "#172640", "#070d16", "#eef5ff", "#a1b3cc", "#2e4364", "#74acff"] },
  forest: { name: "Evergreen", dark: true, colors: ["#08110f", "#101e1a", "#1b302a", "#080e0c", "#edfaf5", "#9abbb0", "#2b4b3f", "#73d8ad"] },
  wine: { name: "Burgundy", dark: true, colors: ["#130a10", "#1f121a", "#311d29", "#0e070b", "#fff0f4", "#c7a2b5", "#4f3041", "#f18fb4"] },
  porcelain: { name: "Porcelain", dark: false, colors: ["#f1f0f7", "#ffffff", "#e7e5f2", "#f6f5fa", "#1f1c2c", "#5e596c", "#c9c5d5", "#6b43cf"] },
} as const;
export type ThemeId = keyof typeof themes;
export const searchBlocks = ["overview", "query", "status", "results"] as const;
export const indexerBlocks = ["overview", "tools", "panel"] as const;
export const navigationPages = ["search", "indexers", "history", "monitoring", "settings"] as const;
export type Workspace = "search" | "indexers";
export type UiPreferences = {
  version: 1;
  theme: ThemeId;
  colors: Partial<Palette>;
  density: "comfortable" | "compact";
  sidebar: "left" | "right" | "top";
  sidebarWidth: number;
  radius: number;
  fontScale: number;
  indexerSplit: number;
  navigation: string[];
  blocks: Record<Workspace, string[]>;
  hiddenBlocks: Record<Workspace, string[]>;
  customCss: string;
};
export const defaults: UiPreferences = {
  version: 1, theme: "obsidian", colors: {}, density: "comfortable",
  sidebar: "left", sidebarWidth: 232, radius: 10, fontScale: 1,
  indexerSplit: 44, navigation: [...navigationPages],
  blocks: { search: [...searchBlocks], indexers: [...indexerBlocks] },
  hiddenBlocks: { search: [], indexers: [] }, customCss: "",
};
const KEY = "indexloom.ui-preferences.v1";
const MAX_CSS = 100_000;

export function paletteFor(preferences: UiPreferences): Palette {
  const theme = themes[preferences.theme];
  return { ...Object.fromEntries(paletteKeys.map((key, index) => [key, theme.colors[index]])) as Palette, ...preferences.colors };
}

function ordered(items: unknown, valid: readonly string[]): string[] {
  const values = Array.isArray(items) ? items.filter((item): item is string => typeof item === "string" && valid.includes(item)) : [];
  return [...new Set([...values, ...valid])];
}
function hidden(items: unknown, valid: readonly string[]): string[] {
  return Array.isArray(items) ? [...new Set(items.filter((item): item is string => typeof item === "string" && valid.includes(item)))] : [];
}
function bounded(value: unknown, low: number, high: number, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.max(low, Math.min(high, value)) : fallback;
}
/** Validate imported profile fields before they affect the page or CSS. */
export function validateProfile(value: unknown): UiPreferences {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Invalid appearance profile");
  const obj = value as Record<string, unknown>;
  if (obj.version !== 1) throw new Error("Unsupported appearance profile version");
  const theme: ThemeId = typeof obj.theme === "string" && Object.hasOwn(themes, obj.theme) ? obj.theme as ThemeId : "obsidian";
  const rawColors = obj.colors && typeof obj.colors === "object" ? obj.colors as Record<string, unknown> : {};
  const colors: Partial<Palette> = {};
  for (const key of paletteKeys) if (typeof rawColors[key] === "string" && /^#[a-f0-9]{6}$/i.test(rawColors[key])) colors[key] = rawColors[key];
  const rawBlocks = obj.blocks && typeof obj.blocks === "object" ? obj.blocks as Record<string, unknown> : {};
  const rawHidden = obj.hiddenBlocks && typeof obj.hiddenBlocks === "object" ? obj.hiddenBlocks as Record<string, unknown> : {};
  const customCss = typeof obj.customCss === "string" ? obj.customCss : "";
  if (customCss.length > MAX_CSS) throw new Error("Custom CSS exceeds 100,000 characters");
  return {
    version: 1, theme, colors,
    density: obj.density === "compact" ? "compact" : "comfortable",
    sidebar: obj.sidebar === "right" || obj.sidebar === "top" ? obj.sidebar : "left",
    sidebarWidth: bounded(obj.sidebarWidth, 170, 350, 232),
    radius: bounded(obj.radius, 0, 24, 10),
    fontScale: bounded(obj.fontScale, 0.85, 1.3, 1),
    indexerSplit: bounded(obj.indexerSplit, 25, 75, 44),
    navigation: ordered(obj.navigation, navigationPages),
    blocks: { search: ordered(rawBlocks.search, searchBlocks), indexers: ordered(rawBlocks.indexers, indexerBlocks) },
    hiddenBlocks: { search: hidden(rawHidden.search, searchBlocks), indexers: hidden(rawHidden.indexers, indexerBlocks) },
    customCss,
  };
}
function initial(): UiPreferences {
  try {
    const saved = localStorage.getItem(KEY);
    return saved ? validateProfile(JSON.parse(saved) as unknown) : defaults;
  } catch { return defaults; }
}
type ContextValue = {
  preferences: UiPreferences;
  update: (fn: (current: UiPreferences) => UiPreferences) => void;
  importProfile: (input: unknown) => void;
  reset: () => void;
};
const Context = createContext<ContextValue | null>(null);
export function AppearanceProvider({ children }: { children: ReactNode }) {
  const [preferences, setPreferences] = useState<UiPreferences>(initial);
  const context = useMemo<ContextValue>(() => ({
    preferences,
    update: (fn) => setPreferences((current) => validateProfile(fn(current))),
    importProfile: (data) => setPreferences(validateProfile(data)),
    reset: () => setPreferences(defaults),
  }), [preferences]);
  useEffect(() => {
    try { localStorage.setItem(KEY, JSON.stringify(preferences)); } catch { /* Storage disabled. */ }
  }, [preferences]);
  useEffect(() => {
    const root = document.documentElement;
    const palette = paletteFor(preferences);
    const variables: Record<PaletteKey, string> = {
      page: "--color-bg-page", surface: "--color-bg-surface",
      raised: "--color-bg-raised", sunken: "--color-bg-sunken",
      text: "--color-text-primary", muted: "--color-text-secondary",
      border: "--color-border-default", accent: "--color-accent-solid",
    };
    for (const key of paletteKeys) root.style.setProperty(variables[key], palette[key]);
    root.style.setProperty("--color-border-subtle", palette.border);
    root.style.setProperty("--color-border-strong", palette.border);
    root.style.setProperty("--color-accent-solid-hover", palette.accent);
    root.style.setProperty("--accent-strong", palette.accent);
    root.style.setProperty("--ui-radius", preferences.radius + "px");
    root.style.setProperty("--ui-sidebar-width", preferences.sidebarWidth + "px");
    root.style.setProperty("--ui-indexer-split", preferences.indexerSplit + "%");
    root.style.setProperty("--ui-density", preferences.density === "compact" ? "0.8" : "1");
    root.style.fontSize = 16 * preferences.fontScale + "px";
    root.dataset.appearance = themes[preferences.theme].dark ? "dark" : "light";
    root.dataset.sidebar = preferences.sidebar;
    root.dataset.density = preferences.density;
    root.dataset.hiddenSearch = preferences.hiddenBlocks.search.join(" ");
    root.dataset.hiddenIndexers = preferences.hiddenBlocks.indexers.join(" ");
    for (const kind of ["search", "indexers"] as const) {
      for (const [index, id] of preferences.blocks[kind].entries()) {
        root.style.setProperty("--" + kind + "-block-" + id + "-order", String(index));
      }
    }
    document.querySelector('meta[name="color-scheme"]')?.setAttribute("content", themes[preferences.theme].dark ? "dark" : "light");
    document.querySelector('meta[name="theme-color"]')?.setAttribute("content", palette.page);
  }, [preferences]);
  useEffect(() => {
    const style = document.createElement("style");
    style.id = "indexloom-user-css";
    style.textContent = preferences.customCss;
    document.head.append(style);
    return () => { style.remove(); };
  }, [preferences.customCss]);
  return <Context.Provider value={context}>{children}</Context.Provider>;
}
export function useAppearance(): ContextValue {
  const context = useContext(Context);
  if (!context) throw new Error("AppearanceProvider is missing");
  return context;
}
export function move(items: string[], from: string, to: string): string[] {
  const source = items.indexOf(from), target = items.indexOf(to);
  if (source < 0 || target < 0 || source === target) return items;
  const result = [...items];
  result.splice(source, 1); result.splice(target, 0, from);
  return result;
}
