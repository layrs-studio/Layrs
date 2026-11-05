import { useCallback, useEffect, useSyncExternalStore } from "react";

export type ThemePreference = "dark" | "light" | "system";
export type ResolvedTheme = "dark" | "light";

export type Appearance = {
  theme: ThemePreference;
  /** Layrs can reduce motion further; it never re-enables motion the OS reduces. */
  reduceMotion: boolean;
};

/**
 * Local, non-sensitive preference. The early script in each `index.html`
 * reads the same key before the first paint; keep both in sync.
 */
export const APPEARANCE_STORAGE_KEY = "layrs.appearance.v1";

const DEFAULT_APPEARANCE: Appearance = { theme: "dark", reduceMotion: false };

let current: Appearance = readStoredAppearance();
let persisted = true;
const listeners = new Set<() => void>();

function readStoredAppearance(): Appearance {
  try {
    const raw = globalThis.localStorage?.getItem(APPEARANCE_STORAGE_KEY);
    if (!raw) return DEFAULT_APPEARANCE;
    const parsed = JSON.parse(raw) as Partial<Appearance>;
    const theme = parsed.theme === "light" || parsed.theme === "system" ? parsed.theme : "dark";
    return { theme, reduceMotion: parsed.reduceMotion === true };
  } catch {
    return DEFAULT_APPEARANCE;
  }
}

function systemTheme(): ResolvedTheme {
  return globalThis.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

export function resolveTheme(theme: ThemePreference): ResolvedTheme {
  return theme === "system" ? systemTheme() : theme;
}

/** Applies the preference to <html>; safe to call repeatedly. */
export function applyAppearance(appearance: Appearance = current) {
  const root = globalThis.document?.documentElement;
  if (!root) return;
  root.dataset.theme = resolveTheme(appearance.theme);
  if (appearance.reduceMotion) root.dataset.motion = "reduced";
  else delete root.dataset.motion;
}

export function setAppearance(next: Partial<Appearance>) {
  current = { ...current, ...next };
  try {
    globalThis.localStorage?.setItem(APPEARANCE_STORAGE_KEY, JSON.stringify(current));
    persisted = true;
  } catch {
    // Storage can be unavailable (private mode, quota): keep the choice for this session.
    persisted = false;
  }
  applyAppearance();
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function snapshot() {
  return current;
}

/** Current preference, whether it survives a restart, and a setter. */
export function useAppearance() {
  const appearance = useSyncExternalStore(subscribe, snapshot, snapshot);

  // "Système" follows OS changes live; explicit choices ignore them.
  useEffect(() => {
    if (appearance.theme !== "system") return;
    const query = globalThis.matchMedia?.("(prefers-color-scheme: light)");
    if (!query) return;
    const onChange = () => applyAppearance();
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, [appearance.theme]);

  const update = useCallback((next: Partial<Appearance>) => setAppearance(next), []);
  return { appearance, persisted, setAppearance: update };
}

/** True when the OS or Layrs asks for reduced motion. */
export function prefersReducedMotion() {
  return (
    current.reduceMotion ||
    Boolean(globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches)
  );
}
