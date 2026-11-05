import { nativeAdminRoute } from "@layrs/ui";
import type { Api } from "./session";

export type Command = { action: string; [key: string]: unknown };

/** Maps the shared collaboration commands to the V3 HTTP routes of one project. */
export function collaborationTransport(api: Api, spaceId: string) {
  const base = `/v3/spaces/${encodeURIComponent(spaceId)}`;
  const part = (value: unknown) => encodeURIComponent(String(value));
  return async function collaboration<T>(command: Command): Promise<T> {
    switch (command.action) {
      case "stepHistory":
        return api<T>(`${base}/history`, "POST", {
          anchor: command.anchor,
          cursor: command.cursor,
          limit: command.limit,
          path: command.path ?? null,
          filter: command.filter ?? {},
        });
      case "listReleases":
        return api<T>(`${base}/releases`);
      case "releasePublications":
        return api<T>(`${base}/releases/${part(command.release)}/publications`);
      case "publishRelease":
        return api<T>(`${base}/releases/${part(command.release)}/publications`, "POST", command.request);
      case "revokeRelease":
        return api<T>(`${base}/releases/${part(command.release)}/publications/${part(command.publication)}/revoke`, "POST", command.request);
      case "listReviews":
        return api<T>(`${base}/reviews`);
      case "listReviewsPage": {
        const query = new URLSearchParams({ limit: String(command.limit) });
        if (command.cursor) query.set("cursor", String(command.cursor));
        return api<T>(`${base}/reviews?${query}`);
      }
      case "review":
        return api<T>(`${base}/reviews/${part(command.review)}`);
      case "propose":
        return api<T>(command.review ? `${base}/reviews/${part(command.review)}/versions` : `${base}/reviews`, "POST", command.request);
      case "decide":
        return api<T>(`${base}/reviews/${part(command.review)}/actions`, "POST", command.request);
      case "compare":
        return api<T>(`${base}/compare`, "POST", { before: command.before, after: command.after });
      case "runDetail":
        return api<T>(`${base}/runs/${part(command.run)}`);
      case "listRuns":
        return api<T>(`${base}/runs`);
      case "cancelRun":
        return api<T>(`${base}/runs/${part(command.run)}/cancel`, "POST", command.request);
      case "correctComment":
        return api<T>(`${base}/reviews/${part(command.review)}/actions/${part(command.comment)}/corrections`, "POST", command.request);
      case "listRunsPage": {
        const query = new URLSearchParams({ limit: String(command.limit ?? 50) });
        if (command.cursor) query.set("cursor", String(command.cursor));
        return api<T>(`${base}/runs?${query}`);
      }
      default:
        throw new Error("Commande de collaboration inconnue.");
    }
  };
}

/** Administration commands; `spaceId` may be empty for instance-level ones. */
export function administrationTransport(api: Api, spaceId: string) {
  return async function administration<T>(command: Command): Promise<T> {
    const route = nativeAdminRoute(command, spaceId);
    return api<T>(route.path, route.method, route.body);
  };
}

export type Bootstrap = {
  spaceId: string;
  layers: { record: { layerId: string; name: string }; headStepIds: string[] }[];
  markers: { record: { name: string; targetStepId: string } }[];
  releases: { record: { version: string; targetStepId: string } }[];
};
