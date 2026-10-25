export type NativeAdminCommand = { action: string; [key: string]: unknown };
export type NativeAdminRoute = { method: "GET" | "POST" | "PUT"; path: string; body?: unknown };
/** Pure allowlisted Web adapter. Authentication belongs to the caller's memory-only transport. */
export function nativeAdminRoute(command: NativeAdminCommand, space: string): NativeAdminRoute {
  const segment = (value: unknown) => {
    if (typeof value !== "string" || !/^[A-Za-z0-9_:-]{1,160}$/.test(value)) throw new Error("Identifiant invalide");
    return encodeURIComponent(value);
  };
  const get = (path: string): NativeAdminRoute => ({ method: "GET", path });
  const post = (path: string, body: unknown = {}): NativeAdminRoute => ({ method: "POST", path, body });
  const number = (value: unknown) => { if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) throw new Error("Position invalide"); return value; };
  switch (command.action) {
    case "organisationSettings": return get("/v3/admin/organisations/settings");
    case "configureOrganisations": return { method: "PUT", path: "/v3/admin/organisations/settings", body: command.request };
    case "retireLens":
    case "revokeLens": {
      const pin = command.pin as { lensId: string; version: string; digest: string };
      if (!pin || !/^[a-zA-Z0-9._-]{1,160}$/.test(pin.lensId) || !/^[a-zA-Z0-9.+_-]{1,80}$/.test(pin.version) || !/^sha256:[a-f0-9]{64}$/.test(pin.digest)) throw Error("Version de lens invalide");
      return post(`/v3/lenses/${encodeURIComponent(pin.lensId)}/${encodeURIComponent(pin.version)}/${encodeURIComponent(pin.digest)}/${command.action === "retireLens" ? "retire" : "revoke"}`);
    }
    case "listLenses": return get("/v3/lenses");
    case "lensCatalog": return get("/v3/lenses/catalog");
    case "installLens": return post("/v3/lenses", command.request);
    case "metrics": return get("/v3/admin/metrics");
    case "invitationInbox": return get(`/v3/invitations?after=${number(command.after)}`);
    case "decideInvitation": {
      if (!["accept", "reject", "revoke"].includes(String(command.decision))) throw new Error("Décision invalide");
      return post(`/v3/invitations/${segment(command.invitation)}/actions`, { action: command.decision });
    }
    case "storageBudget": return get(`/v3/admin/storage-budget${command.space ? `?spaceId=${segment(command.space)}` : ""}`);
    case "listAccounts": return get("/v3/auth/accounts");
    case "createAccount": return post("/v3/auth/accounts", command.request);
    case "revokeAccount": return post("/v3/auth/accounts/revoke", { principalId: segment(command.principalId).replace(/%3A/g, ":") });
    case "createService": return post("/v3/auth/service-accounts", command.request);
    case "rotateService": return post("/v3/auth/service-accounts/rotate", command.request);
    case "listTokens": return get("/v3/auth/tokens");
    case "revokeToken": return post("/v3/auth/tokens/revoke", { tokenId: command.tokenId });
  }
  const base = `/v3/spaces/${segment(space)}`;
  switch (command.action) {
    case "projectLenses": return get(`${base}/lenses`);
    case "filePreview": return post(`${base}/preview`, command.request);
    case "configureLenses": return { method: "PUT", path: `${base}/lenses`, body: command.request };
    case "lensPacket": {
      const pin = command.pin as { lensId: string; version: string; digest: string };
      if (!pin || !/^[a-zA-Z0-9._-]{1,160}$/.test(pin.lensId) || !/^[a-zA-Z0-9.+_-]{1,80}$/.test(pin.version) || !/^sha256:[a-f0-9]{64}$/.test(pin.digest)) throw Error("Version de lens invalide");
      return get(`${base}/lenses/${encodeURIComponent(pin.lensId)}/${encodeURIComponent(pin.version)}/${encodeURIComponent(pin.digest)}`);
    }
    case "listTeams": return get(`${base}/teams`);
    case "listOrganisationTeams": return get(`${base}/organisation-teams`);
    case "createOrganisationTeam": return post(`${base}/organisation-teams`, command.request);
    case "updateOrganisationTeam": return {method:"PUT",path:`${base}/organisation-teams/${segment(command.team)}`,body:command.request};
    case "archiveOrganisationTeam": return post(`${base}/organisation-teams/${segment(command.team)}/archive`,command.request);
    case "assignOrganisationTeam": return {method:"PUT",path:`${base}/organisation-teams/${segment(command.team)}/assignment`,body:command.request};
    case "policyManagement": return get(`${base}/policy-management`);
    case "eligibleTeamMembers": return get(`${base}/teams/eligible-members`);
    case "createTeam": return post(`${base}/teams`, command.request);
    case "updateTeam": return {method:"PUT",path:`${base}/teams/${segment(command.team)}`,body:command.request};
    case "archiveTeam": return post(`${base}/teams/${segment(command.team)}/archive`,command.request);
    case "teamHistory": return get(`${base}/teams/${segment(command.team)}/history?after=${number(command.after)}&limit=${number(command.limit)}${command.anchor == null ? "" : `&anchor=${number(command.anchor)}`}`);
    case "listInvitations": return get(`${base}/invitations?after=${number(command.after)}`);
    case "inviteMember": return post(`${base}/invitations`, command.request);
    case "grantMember": return post(`${base}/members`, command.request);
    case "protection": return get(`${base}/layers/${segment(command.layer)}/protection`);
    case "configureProtection": return { method: "PUT", path: `${base}/layers/${segment(command.layer)}/protection`, body: command.request };
    case "listWebhooks": return get(`${base}/webhooks`);
    case "createWebhook": return post(`${base}/webhooks`, command.request);
    case "webhookDeliveries": return get(`${base}/webhooks/${segment(command.webhook)}/deliveries`);
    case "rotateWebhook": return post(`${base}/webhooks/${segment(command.webhook)}/rotate`);
    case "disableWebhook": return post(`${base}/webhooks/${segment(command.webhook)}/disable`);
    case "replayWebhook": return post(`${base}/webhooks/${segment(command.webhook)}/replay`, { deliveryId: command.deliveryId });
    case "notifications": return get(`${base}/notifications?after=${number(command.after)}`);
    case "markNotificationsRead": return post(`${base}/notifications/read`, { lastSequence: number(command.lastSequence) });
    default: throw new Error("Action d’administration inconnue");
  }
}
