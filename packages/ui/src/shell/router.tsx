import { useEffect, useSyncExternalStore, type AnchorHTMLAttributes, type MouseEvent, type ReactNode } from "react";

/*
 * Hash routing: `#/projects/abc/changes`. It works unchanged in Tauri, in the
 * HTTPS test proxy and on any static host, with no server rewrite rule.
 * Routes never carry secrets.
 */

export type Location = { path: string; query: URLSearchParams };

function readLocation(): Location {
  const raw = globalThis.location?.hash.replace(/^#/, "") || "/";
  const [path, search = ""] = raw.split("?", 2);
  return { path: normalizePath(path), query: new URLSearchParams(search) };
}

function normalizePath(path: string) {
  const trimmed = `/${path}`.replace(/\/{2,}/g, "/").replace(/(.)\/$/, "$1");
  return trimmed || "/";
}

let location = readLocation();
let locationKey = globalThis.location?.hash ?? "";
const listeners = new Set<() => void>();

/** Pages with unsaved work register a message; leaving asks for confirmation. */
const guards = new Set<{ message: string }>();

function leaveAllowed() {
  for (const guard of guards) if (!globalThis.confirm?.(guard.message)) return false;
  return true;
}

function refresh(confirmed = false) {
  const hash = globalThis.location?.hash ?? "";
  if (hash === locationKey) return;
  if (!confirmed && guards.size && !leaveAllowed()) {
    // Back/forward or a typed address: return to the page being edited.
    globalThis.history.pushState(globalThis.history.state, "", locationKey || "#/");
    return;
  }
  locationKey = hash;
  location = readLocation();
  listeners.forEach((listener) => listener());
}

globalThis.addEventListener?.("hashchange", () => refresh());
globalThis.addEventListener?.("popstate", () => refresh());

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useLocation(): Location {
  return useSyncExternalStore(subscribe, () => location, () => location);
}

export function href(path: string, query?: Record<string, string | undefined>) {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(query ?? {})) if (value) search.set(key, value);
  const suffix = search.toString();
  return `#${normalizePath(path)}${suffix ? `?${suffix}` : ""}`;
}

export function navigate(path: string, options: { replace?: boolean; query?: Record<string, string | undefined> } = {}) {
  const target = href(path, options.query);
  if (target === locationKey) return;
  if (guards.size && !leaveAllowed()) return;
  if (options.replace) globalThis.history.replaceState(globalThis.history.state, "", target);
  else globalThis.history.pushState(globalThis.history.state, "", target);
  refresh(true);
}

/**
 * While `message` is set, leaving the page (in-app link, back/forward, reload
 * or closing the tab) asks for confirmation instead of losing the work.
 */
export function useNavigationGuard(message: string | null) {
  useEffect(() => {
    if (!message) return;
    const guard = { message };
    const unload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = "";
    };
    guards.add(guard);
    globalThis.addEventListener?.("beforeunload", unload);
    return () => {
      guards.delete(guard);
      globalThis.removeEventListener?.("beforeunload", unload);
    };
  }, [message]);
}

/**
 * Matches `/projects/:project/reviews/:review` style patterns. A trailing
 * `/*` matches any remainder, exposed as the `*` parameter.
 */
export function matchRoute(pattern: string, path: string): Record<string, string> | null {
  const patternParts = normalizePath(pattern).split("/").filter(Boolean);
  const pathParts = normalizePath(path).split("/").filter(Boolean);
  const params: Record<string, string> = {};
  for (let index = 0; index < patternParts.length; index++) {
    const part = patternParts[index];
    if (part === "*") {
      params["*"] = pathParts.slice(index).join("/");
      return params;
    }
    const value = pathParts[index];
    if (value === undefined) return null;
    if (part.startsWith(":")) {
      try {
        params[part.slice(1)] = decodeURIComponent(value);
      } catch {
        return null;
      }
    } else if (part !== value) {
      return null;
    }
  }
  return patternParts.length === pathParts.length ? params : null;
}

/** Encodes one dynamic segment, e.g. a `space:...` key. */
export function segment(value: string) {
  return encodeURIComponent(value);
}

type LinkProps = Omit<AnchorHTMLAttributes<HTMLAnchorElement>, "href"> & {
  to: string;
  query?: Record<string, string | undefined>;
  replace?: boolean;
  children: ReactNode;
};

export function Link({ to, query, replace, onClick, children, ...props }: LinkProps) {
  const target = href(to, query);
  function handleClick(event: MouseEvent<HTMLAnchorElement>) {
    onClick?.(event);
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    navigate(to, { replace, query });
  }
  return (
    <a href={target} onClick={handleClick} {...props}>
      {children}
    </a>
  );
}
