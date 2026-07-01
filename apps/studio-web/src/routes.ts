export type StudioRoute =
  | { name: "home" }
  | { name: "team"; teamId: string }
  | { name: "space"; spaceId: string; layerId?: string };

export function parseStudioRoute(pathname: string): StudioRoute {
  const parts = pathname
    .replace(/\/+$/, "")
    .split("/")
    .filter(Boolean)
    .map((part) => decodeURIComponent(part));

  if (parts[0] === "teams" && parts[1]) {
    return { name: "team", teamId: parts[1] };
  }

  if (parts[0] === "spaces" && parts[1]) {
    if (parts[2] === "layers" && parts[3]) {
      return { name: "space", spaceId: parts[1], layerId: parts[3] };
    }

    return { name: "space", spaceId: parts[1] };
  }

  return { name: "home" };
}

export function currentStudioRoute(): StudioRoute {
  return parseStudioRoute(globalThis.location?.pathname ?? "/");
}

export function homeHref(): string {
  return "/";
}

export function teamHref(teamId: string): string {
  return `/teams/${encodeURIComponent(teamId)}`;
}

export function spaceHref(spaceId: string): string {
  return `/spaces/${encodeURIComponent(spaceId)}`;
}

export function layerHref(spaceId: string, layerId: string): string {
  return `/spaces/${encodeURIComponent(spaceId)}/layers/${encodeURIComponent(layerId)}`;
}

export function routeHref(route: StudioRoute): string {
  if (route.name === "team") {
    return teamHref(route.teamId);
  }

  if (route.name === "space") {
    return route.layerId ? layerHref(route.spaceId, route.layerId) : spaceHref(route.spaceId);
  }

  return homeHref();
}

export function routeLabel(route: StudioRoute): string {
  if (route.name === "team") {
    return "Team";
  }

  if (route.name === "space") {
    return "Space";
  }

  return "Workspace";
}
