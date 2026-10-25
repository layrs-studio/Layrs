import type { Api, Principal } from "./session";

/** Same-origin, tab-local device authorization. No credential is sent over this channel. */
const CHANNEL = "layrs.device-authorization.v1";
const MAX_IN_FLIGHT = 4;
const MAX_TICKETS = 32;
const MAX_REQUESTS_PER_MINUTE = 64;
const TICKET_LIFETIME_MS = 60_000;
const requestIdPattern = /^[A-Za-z0-9_-]{8,128}$/;
const userCodePattern = /^(?:[A-Fa-f0-9]{10}|[A-Fa-f0-9]{5}-[A-Fa-f0-9]{5})$/;

type Ticket = { userCode: string; expiresAt: number };
type DeviceDetails = { label: string; expiresAt: string };

export function startDeviceAuthorizationBridge(options: {
  api: Api;
  principal: Principal;
  isCurrent: () => boolean;
}): () => void {
  if (typeof BroadcastChannel === "undefined" || !options.isCurrent()) return () => {};

  const providerId = crypto.randomUUID();
  let channel: BroadcastChannel;
  try { channel = new BroadcastChannel(CHANNEL); }
  catch { return () => {}; }
  const tickets = new Map<string, Ticket>();
  const inFlight = new Set<string>();
  const requestTimes: number[] = [];
  let disposed = false;
  const current = () => !disposed && options.isCurrent();

  const reply = (requestId: string, ok: boolean, data?: object, error?: string) => {
    if (!current()) return;
    channel.postMessage({ type: "result", requestId, providerId, ok, ...(ok ? { data } : { error }) });
  };

  channel.onmessage = (event: MessageEvent<unknown>) => {
    if (!current() || !event.data || typeof event.data !== "object" || Array.isArray(event.data)) return;
    const message = event.data as Record<string, unknown>;
    const { type, requestId } = message;
    if (typeof requestId !== "string" || !requestIdPattern.test(requestId)) return;
    if (type !== "discover" && message.providerId !== providerId) return;
    if (type !== "discover" && type !== "inspect" && type !== "approve") return;

    const now = Date.now();
    while (requestTimes.length && requestTimes[0] <= now - 60_000) requestTimes.shift();
    if (requestTimes.length >= MAX_REQUESTS_PER_MINUTE) return;
    requestTimes.push(now);
    for (const [id, ticket] of tickets) if (ticket.expiresAt <= now) tickets.delete(id);

    if (type === "discover") {
      channel.postMessage({
        type: "offer", requestId, providerId,
        principal: { principalId: options.principal.principalId, displayName: options.principal.displayName },
      });
      return;
    }
    if (inFlight.size >= MAX_IN_FLIGHT || inFlight.has(requestId)) {
      reply(requestId, false, undefined, "Une vérification est déjà en cours. Réessayez dans un instant.");
      return;
    }

    if (type === "inspect") {
      if (typeof message.userCode !== "string" || !userCodePattern.test(message.userCode)) {
        reply(requestId, false, undefined, "Saisissez le code à dix caractères affiché dans l’application.");
        return;
      }
      if (tickets.size >= MAX_TICKETS) {
        reply(requestId, false, undefined, "Trop de vérifications sont en cours. Réessayez dans un instant.");
        return;
      }
      const userCode = message.userCode.replace("-", "").toUpperCase();
      inFlight.add(requestId);
      void (async () => {
        try {
          const details = await options.api<DeviceDetails>("/v3/auth/devices/inspect", "POST", { userCode });
          if (!current()) return;
          const deviceExpiry = Date.parse(details.expiresAt);
          if (typeof details.label !== "string" || details.label.length > 200 || !Number.isFinite(deviceExpiry) || deviceExpiry <= Date.now()) {
            throw new Error("Réponse de vérification invalide.");
          }
          const ticket = crypto.randomUUID();
          tickets.set(ticket, { userCode, expiresAt: Math.min(Date.now() + TICKET_LIFETIME_MS, deviceExpiry) });
          reply(requestId, true, { label: details.label, expiresAt: details.expiresAt, ticket });
        } catch {
          reply(requestId, false, undefined, "Code invalide ou expiré. Vérifiez le code dans l’application et réessayez.");
        } finally {
          inFlight.delete(requestId);
        }
      })();
      return;
    }

    const ticket = typeof message.ticket === "string" && message.ticket.length <= 128 ? tickets.get(message.ticket) : undefined;
    // One approval attempt per inspection, even when the network fails.
    if (typeof message.ticket === "string") tickets.delete(message.ticket);
    if (!ticket || ticket.expiresAt <= Date.now()) {
      reply(requestId, false, undefined, "Cette vérification a expiré. Vérifiez de nouveau le code avant d’autoriser l’appareil.");
      return;
    }
    inFlight.add(requestId);
    void (async () => {
      try {
        await options.api("/v3/auth/devices/approve", "POST", { userCode: ticket.userCode });
        if (current()) reply(requestId, true, {});
      } catch {
        reply(requestId, false, undefined, "La confirmation n’a pas été reçue. Vérifiez Desktop avant de recommencer ; l’autorisation a pu aboutir.");
      } finally {
        inFlight.delete(requestId);
      }
    })();
  };

  return () => {
    disposed = true;
    channel.close();
    tickets.clear();
    inFlight.clear();
  };
}
