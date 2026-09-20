"use strict";
const form = document.getElementById("login");
const confirmation = document.getElementById("confirmation");
const status = document.getElementById("status");
let token = null;
let userCode = null;
async function call(path, body, authenticated = false) {
  const headers = { "Content-Type": "application/json" };
  if (authenticated) headers.Authorization = `Bearer ${token}`;
  const response = await fetch(path, { method: "POST", headers, body: JSON.stringify(body), credentials: "omit", cache: "no-store" });
  if (!response.ok) throw new Error(response.status === 429 ? "Trop de tentatives. Réessayez plus tard." : "Demande refusée ou expirée. Vérifiez vos informations.");
  return response.status === 204 ? null : response.json();
}
async function logout() {
  if (token) { try { await call("/v3/auth/logout", {}, true); } catch (_) { /* Still clear local credentials. */ } }
  token = null;
}
form.addEventListener("submit", async event => {
  event.preventDefault();
  const button = form.querySelector("button");
  button.disabled = true;
  status.textContent = "Vérification…";
  try {
    await logout();
    const password = document.getElementById("password");
    const credentials = { login: document.getElementById("account").value, password: password.value };
    password.value = "";
    token = (await call("/v3/auth/login", credentials)).access_token;
    userCode = document.getElementById("code").value;
    const device = await call("/v3/auth/devices/inspect", { userCode }, true);
    document.getElementById("device").textContent = `Appareil : ${device.label}`;
    confirmation.hidden = false; form.hidden = true; status.textContent = "Vérifiez le nom de votre appareil avant de confirmer.";
  } catch (error) { await logout(); status.textContent = error.message; }
  finally { button.disabled = false; }
});
document.getElementById("approve").addEventListener("click", async () => {
  const button = document.getElementById("approve"); button.disabled = true;
  try { await call("/v3/auth/devices/approve", { userCode }, true); status.textContent = "Appareil autorisé. Vous pouvez revenir à Layrs."; confirmation.hidden = true; }
  catch (error) { status.textContent = error.message; confirmation.hidden = true; form.hidden = false; }
  finally { await logout(); button.disabled = false; }
});
document.getElementById("cancel").addEventListener("click", async () => {
  await logout(); confirmation.hidden = true; form.hidden = false; status.textContent = "Autorisation annulée.";
});
