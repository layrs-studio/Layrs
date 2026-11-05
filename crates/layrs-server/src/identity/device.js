"use strict";
(() => {
  const byId = id => document.getElementById(id);
  byId("instance").textContent = location.host;
  const form = byId("login"), fields = byId("fields"), confirmation = byId("confirmation");
  const status = byId("status"), session = byId("session"), password = byId("password");
  const offers = new Map(), pending = new Map();
  let token = null, inspected = null, discovery = null, channel = null;
  try { channel = new BroadcastChannel("layrs.device-authorization.v1"); } catch (_) { /* Manual login remains available. */ }
  const errorText = error => error instanceof Error ? error.message : "La demande n'a pas pu être vérifiée.";
  async function call(path, body, bearer = null) {
    const headers = { "Content-Type": "application/json" };
    if (bearer) headers.Authorization = `Bearer ${bearer}`;
    const response = await fetch(path, { method: "POST", headers, body: JSON.stringify(body), credentials: "omit", cache: "no-store", redirect: "error", signal: AbortSignal.timeout(15000) });
    if (!response.ok) throw new Error(response.status === 429 ? "Trop de tentatives. Réessayez plus tard." : "Demande refusée ou expirée. Vérifiez vos informations et le code de l'appareil.");
    return response.status === 204 ? null : response.json();
  }
  async function logout() {
    // Revoke only this page's temporary login, never a Web tab's session.
    const previous = token;
    token = null;
    if (previous) { try { await call("/v3/auth/logout", {}, previous); } catch (_) { /* Still clear local credentials. */ } }
  }
  function credentials() {
    const manual = session.value === "manual";
    byId("credentials").hidden = !manual;
    byId("account").disabled = password.disabled = !manual;
    byId("account").required = password.required = manual;
    password.value = "";
  }
  function rpc(providerId, type, body) {
    return new Promise((resolve, reject) => {
      const requestId = crypto.randomUUID();
      const timer = setTimeout(() => {
        pending.delete(requestId);
        reject(new Error(type === "approve" ? "La confirmation n'a pas été reçue. Vérifiez Desktop avant de recommencer ; l'autorisation a pu aboutir." : "L'onglet connecté ne répond plus. Recherchez les onglets ou connectez-vous avec un compte."));
      }, 15000);
      pending.set(requestId, { providerId, resolve, reject, timer });
      channel.postMessage({ type, requestId, providerId, ...body });
    });
  }
  if (channel) channel.onmessage = ({ data }) => {
    if (!data || typeof data !== "object") return;
    if (data.type === "offer" && data.requestId === discovery && offers.size < 16 && typeof data.providerId === "string" && data.providerId.length <= 100 && typeof data.principal?.principalId === "string" && data.principal.principalId.length <= 160 && typeof data.principal.displayName === "string" && data.principal.displayName.length <= 200) {
      if (![...offers.values()].some(offer => offer.principal.principalId === data.principal.principalId)) offers.set(data.providerId, data);
    }
    const request = pending.get(data.requestId);
    if (data.type !== "result" || !request || data.providerId !== request.providerId) return;
    pending.delete(data.requestId); clearTimeout(request.timer);
    if (data.ok === true) request.resolve(data.data);
    else request.reject(new Error(typeof data.error === "string" ? data.error.slice(0, 500) : "La session a changé. Recherchez les onglets connectés."));
  };
  async function discover() {
    fields.disabled = true; offers.clear(); inspected = null;
    status.textContent = "Recherche d'un compte connecté…";
    if (channel) {
      discovery = crypto.randomUUID();
      channel.postMessage({ type: "discover", requestId: discovery });
      await new Promise(resolve => setTimeout(resolve, 800));
      discovery = null;
    }
    session.replaceChildren();
    if (offers.size > 1) session.add(new Option("Choisir le compte à utiliser…", ""));
    for (const [id, offer] of offers) session.add(new Option(`${offer.principal.displayName || "Compte connecté"} — ${offer.principal.principalId}`, id));
    session.add(new Option("Se connecter avec un compte", "manual"));
    session.value = offers.size === 1 ? offers.keys().next().value : offers.size > 1 ? "" : "manual";
    byId("session-help").textContent = offers.size ? "Compte disponible dans un autre onglet de cette instance. Gardez cet onglet ouvert jusqu'à la confirmation." : "Aucun onglet connecté trouvé. Connectez-vous ici, ou ouvrez Studio sur cette même adresse puis relancez la recherche.";
    credentials(); fields.disabled = false; status.textContent = "";
  }
  session.addEventListener("change", credentials);
  byId("refresh").addEventListener("click", () => void discover());
  form.addEventListener("submit", async event => {
    event.preventDefault();
    if (fields.disabled || !session.value) return;
    const providerId = session.value, userCode = byId("code").value;
    fields.disabled = true; status.textContent = "Vérification…";
    try {
      await logout();
      let device, account;
      if (providerId === "manual") {
        const login = byId("account").value;
        const credentials = { login, password: password.value }; password.value = "";
        token = (await call("/v3/auth/login", credentials)).access_token;
        device = await call("/v3/auth/devices/inspect", { userCode }, token);
        account = login;
      } else {
        const offer = offers.get(providerId);
        if (!offer) throw new Error("Choisissez un compte connecté.");
        device = await rpc(providerId, "inspect", { userCode });
        account = `${offer.principal.displayName || "Compte connecté"} — ${offer.principal.principalId}`;
      }
      inspected = { providerId, userCode, ticket: device.ticket };
      byId("identity").textContent = `Compte : ${account}`;
      byId("device").textContent = `Appareil : ${device.label}`;
      confirmation.hidden = false; form.hidden = true;
      status.textContent = "Vérifiez le compte et le nom de votre appareil avant de confirmer.";
    } catch (error) { await logout(); inspected = null; status.textContent = errorText(error); }
    finally { fields.disabled = false; }
  });
  byId("approve").addEventListener("click", async () => {
    const request = inspected;
    if (!request) return;
    inspected = null; fields.disabled = byId("approve").disabled = byId("cancel").disabled = true;
    let failed = false;
    status.textContent = "Autorisation en cours…";
    try {
      if (request.providerId === "manual") await call("/v3/auth/devices/approve", { userCode: request.userCode }, token);
      else await rpc(request.providerId, "approve", { ticket: request.ticket });
      status.textContent = "Appareil autorisé. Vous pouvez revenir à Layrs.";
      confirmation.hidden = true;
    } catch (error) {
      status.textContent = `${errorText(error)} Vérifiez Desktop avant de recommencer ; si la connexion a été interrompue, l'autorisation a pu aboutir.`;
      failed = true; confirmation.hidden = true;
    }
    finally {
      await logout(); fields.disabled = byId("approve").disabled = byId("cancel").disabled = false;
      if (failed) form.hidden = false;
    }
  });
  byId("cancel").addEventListener("click", async () => {
    if (!inspected) return;
    inspected = null; fields.disabled = byId("approve").disabled = byId("cancel").disabled = true;
    await logout(); confirmation.hidden = true; form.hidden = false; status.textContent = "Autorisation annulée.";
    fields.disabled = byId("approve").disabled = byId("cancel").disabled = false;
  });
  void discover();
})();
