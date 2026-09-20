import { useEffect, useRef, useState } from "react";
import { NativeStoragePanel } from "./native-storage";
import { NativeMetricsPanel } from "./native-metrics";
import { NativeInvitationsSurface } from "./native-invitations";
import { NativeTeamsSurface } from "./native-teams";
import { NativePolicyManagement, type PolicyManagement } from "./native-policy-management";
import { NativePolicyEditor, type PolicyTransport } from "./native-policy-editor";
import { validReviewContext, type NativeReviewContext } from "./native-review-context";

type Data = Record<string, unknown>;
export { nativeAdminRoute } from "./native-admin-route";
export type { NativeAdminCommand, NativeAdminRoute } from "./native-admin-route";
import type { NativeAdminCommand } from "./native-admin-route";

type Account = { principalId: string; displayName: string; kind: string; status: string };
type Token = { tokenId: string; label: string; kind: string; expiresAt: string; revoked: boolean };
type Hook = { webhookId: string; url: string; eventKinds: string[]; secretGeneration: number; enabled: boolean };
type Delivery = { deliveryId: string; state: string; attempts: number; lastStatus: number | null; lastError: string | null };
type Notice = { sequence: number; kind: string; subjectId: string; createdAt: string; reviewContext?: NativeReviewContext | null };
type Protection = { minimumApprovals: number; allowSelfApproval: boolean; rejectConflicts: boolean; recipeObjectId: string | null; trustedRunnerPrincipalIds: string[]; maximumProofAgeSeconds: number };
const eventNames: Record<string, string> = { "review.proposed": "Nouvelle version de revue", "review.action": "Décision ou commentaire", "review.integrated": "Revue intégrée", "review.comment_corrected": "Commentaire corrigé", "proof.completed": "Résultat CI", "proof.cancelled": "Annulation CI", "protection.changed": "Protection modifiée", "membership.changed": "Accès modifié" };

export function NativeAdminSurface({ scope, transport, busy, onBusy, principalKind, onOpenReview, policyTransport }: {
  scope: string;
  transport: <T>(command: NativeAdminCommand) => Promise<T>;
  busy: boolean;
  onBusy: (value: boolean) => void;
  principalKind?: string;
  onOpenReview?: (context: NativeReviewContext) => void;
  policyTransport?: PolicyTransport;
}) {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [policy, setPolicy] = useState<PolicyManagement | null>(null);
  const [tokens, setTokens] = useState<Token[]>([]);
  const [account, setAccount] = useState({ login: "", displayName: "", password: "" });
  const [serviceName, setServiceName] = useState("");
  const [serviceLabel, setServiceLabel] = useState("");
  const [serviceDays, setServiceDays] = useState(7);
  const [principal, setPrincipal] = useState("");
  const [role, setRole] = useState("member");
  const [layer, setLayer] = useState("");
  const [approvals, setApprovals] = useState(1);
  const [selfApproval, setSelfApproval] = useState(false);
  const [rejectConflicts, setRejectConflicts] = useState(true);
  const [requireCI, setRequireCI] = useState(false);
  const [recipe, setRecipe] = useState("");
  const [recipeId, setRecipeId] = useState<string | null>(null);
  const [runners, setRunners] = useState("");
  const [proofHours, setProofHours] = useState(24);
  const [hooks, setHooks] = useState<Hook[]>([]);
  const [hookUrl, setHookUrl] = useState("");
  const [hookEvents, setHookEvents] = useState(["review.proposed", "proof.completed"]);
  const [deliveryHook, setDeliveryHook] = useState("");
  const [deliveries, setDeliveries] = useState<Delivery[]>([]);
  const [notices, setNotices] = useState<Notice[]>([]);
  const [nextAfter, setNextAfter] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [lastRead, setLastRead] = useState(0);
  const [secret, setSecret] = useState<{ title: string; value: string; reference: string } | null>(null);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [confirmation, setConfirmation] = useState<{ label: string; work: () => Promise<void> } | null>(null);
  const generation = useRef(0);
  const inFlight = useRef(false);
  const selectedAccount = accounts.find(a => a.principalId === principal);
  const deploymentAdmin = !principalKind || principalKind === "bootstrap_admin";

  useEffect(() => {
    generation.current += 1;
    setAccounts([]); setTokens([]); setHooks([]); setDeliveries([]); setNotices([]); setPolicy(null);
    setSecret(null); setConfirmation(null); setError(""); setNotice(""); setPrincipal("");
    setAccount({ login: "", displayName: "", password: "" }); setServiceName(""); setServiceLabel("");
    setLayer(""); setRecipe(""); setRecipeId(null); setRequireCI(false); setRunners(""); setHookUrl("");
    setNextAfter(0); setLastRead(0); setHasMore(false);
    return () => { generation.current += 1; };
  }, [scope]);
  useEffect(() => { if (!secret) return; const timer = setTimeout(() => setSecret(null), 60_000); return () => clearTimeout(timer); }, [secret]);
  useEffect(() => { if (!account.password) return; const timer = setTimeout(() => setAccount(a => ({ ...a, password: "" })), 60_000); return () => clearTimeout(timer); }, [account.password]);

  async function request<T>(command: NativeAdminCommand): Promise<T> {
    const current = generation.current;
    const value = await transport<T>(command);
    if (current !== generation.current) throw new Error("Session modifiée");
    return value;
  }
  async function perform(work: () => Promise<void>) {
    if (busy || inFlight.current) return;
    const current = generation.current;
    inFlight.current = true; onBusy(true); setError(""); setNotice(""); setSecret(null); setConfirmation(null);
    try { await work(); }
    catch { if (current === generation.current) setError("L’action a échoué. Vérifiez vos droits, les valeurs saisies et la connexion au serveur."); }
    finally { inFlight.current = false; onBusy(false); }
  }
  function reveal(value: Data, title: string, reference: string) {
    const issued = typeof value.access_token === "string" ? value.access_token : value.secret;
    if (typeof issued !== "string" || !issued) throw new Error("Secret absent de la réponse");
    setSecret({ title, value: issued, reference });
  }
  async function loadAccounts() { setAccounts((await request<{ accounts: Account[] }>({ action: "listAccounts" })).accounts); }
  async function loadHooks() { setHooks((await request<{ webhooks: Hook[] }>({ action: "listWebhooks" })).webhooks); }
  async function loadDeliveries(webhook: string) { setDeliveries((await request<{ deliveries: Delivery[] }>({ action: "webhookDeliveries", webhook })).deliveries); setDeliveryHook(webhook); }
  async function loadNotices(after = 0) {
    const value = await request<{ notifications: Notice[]; nextAfter: number; hasMore: boolean; lastReadSequence: number }>({ action: "notifications", after });
    setNotices(previous => after ? [...previous, ...value.notifications.filter(n => !previous.some(p => p.sequence === n.sequence))] : value.notifications);
    setNextAfter(value.nextAfter); setHasMore(value.hasMore); setLastRead(value.lastReadSequence);
  }
  async function saveProtection() {
    const trustedRunnerPrincipalIds = requireCI ? runners.split(",").map(v => v.trim()).filter(Boolean) : [];
    if (requireCI && (!recipe.trim() || !trustedRunnerPrincipalIds.length)) throw new Error("Recette et exécuteur requis");
    const value = await request<Protection>({ action: "configureProtection", layer, request: { minimumApprovals: approvals, allowSelfApproval: selfApproval, rejectConflicts, recipe: requireCI ? JSON.parse(recipe) : null, trustedRunnerPrincipalIds, maximumProofAgeSeconds: Math.round(proofHours * 3600) } });
    setRecipeId(value.recipeObjectId); setNotice("Protection enregistrée. Le serveur l’applique aux écritures directes et aux publications concernées.");
  }
  async function readProtection() {
    const value = await request<Protection | null>({ action: "protection", layer });
    if (!value) { setNotice("Aucune protection configurée pour ce Layer."); setRecipeId(null); return; }
    setApprovals(value.minimumApprovals); setSelfApproval(value.allowSelfApproval); setRejectConflicts(value.rejectConflicts);
    setRequireCI(Boolean(value.recipeObjectId)); setRecipeId(value.recipeObjectId); setRecipe("");
    setRunners(value.trustedRunnerPrincipalIds.join(", ")); setProofHours(value.maximumProofAgeSeconds / 3600);
    setNotice(value.recipeObjectId ? "Protection chargée. Pour la modifier, fournissez la recette exacte ou désactivez explicitement l’exigence CI." : "Protection chargée.");
  }
  const button = (label: string, work: () => Promise<void>, disabled = false) => <button type="button" disabled={busy || disabled} onClick={() => void perform(work)}>{label}</button>;
  const confirm = (label: string, work: () => Promise<void>) => setConfirmation({ label, work });

  return <details><summary>Équipe, protections et intégrations</summary>
    <NativeInvitationsSurface scope={scope} transport={transport} busy={busy} onBusy={onBusy} />
    <NativeTeamsSurface key={`teams:${scope}`} scope={scope} transport={transport} busy={busy} onBusy={onBusy} onPolicy={setPolicy} globalPolicy={policy} />
    {deploymentAdmin && <NativeStoragePanel key={scope} transport={transport} disabled={busy} />}
    {deploymentAdmin && <NativeMetricsPanel key={`metrics:${scope}`} transport={transport} disabled={busy} />}
    {error && <p className="native-error" role="alert">{error}</p>}
    {notice && <p role="status">{notice}</p>}
    {secret && <section className="native-preview" aria-label="Secret créé"><h3>{secret.title}</h3><code>{secret.reference}</code><p>Copiez ce secret maintenant dans le coffre de l’outil concerné. Il sera masqué après 60 secondes et ne sera pas conservé par cette interface.</p><label>Secret<input type="password" readOnly value={secret.value} autoComplete="off" spellCheck={false} /></label><div className="native-buttons"><button type="button" onClick={() => { void navigator.clipboard.writeText(secret.value).then(() => setNotice("Secret copié."), () => setError("Copie impossible ; sélectionnez le champ du secret.")); }}>Copier le secret</button><button type="button" onClick={() => setSecret(null)}>J’ai enregistré le secret</button></div></section>}
    {confirmation && <section className="native-preview" role="alertdialog" aria-label="Confirmer l’action"><p>{confirmation.label}</p><div className="native-buttons">{button("Confirmer", confirmation.work)}<button type="button" disabled={busy} onClick={() => setConfirmation(null)}>Annuler</button></div></section>}

    <details><summary>Notifications du projet</summary>{button("Actualiser les notifications", () => loadNotices())}
      {notices.map(item => <article key={item.sequence}><strong>{eventNames[item.kind] ?? item.kind}{item.sequence > lastRead ? " · non lu" : ""}</strong><code>{item.subjectId}</code><small>{item.createdAt}</small>{onOpenReview && validReviewContext(item.reviewContext) && <button type="button" disabled={busy} onClick={() => onOpenReview({ ...item.reviewContext! })}>Ouvrir la revue · version {item.reviewContext.version}</button>}</article>)}
      <div className="native-buttons">{hasMore && button("Charger la suite", () => loadNotices(nextAfter))}{button("Marquer ces notifications comme lues", async () => { const value = await request<{ lastReadSequence: number }>({ action: "markNotificationsRead", lastSequence: nextAfter }); setLastRead(value.lastReadSequence); }, !notices.length)}</div>
    </details>

    {deploymentAdmin && <details><summary>Comptes et automatisations</summary>{button("Charger les comptes", loadAccounts)}
      {accounts.map(item => <article key={item.principalId}><strong>{item.displayName}</strong> · {item.kind === "service" ? "Automatisation" : item.kind === "bootstrap_admin" ? "Administrateur" : "Compte"} · {item.status}<code>{item.principalId}</code><div className="native-buttons"><button type="button" disabled={busy || item.status !== "active"} onClick={() => setPrincipal(item.principalId)}>Sélectionner</button>{item.kind !== "bootstrap_admin" && item.status === "active" && <button type="button" disabled={busy} onClick={() => confirm(`Révoquer le compte ${item.displayName} et tous ses jetons ?`, async () => { await request({ action: "revokeAccount", principalId: item.principalId }); await loadAccounts(); setNotice("Compte révoqué."); })}>Révoquer le compte</button>}</div></article>)}
      <form autoComplete="off" onSubmit={e => { e.preventDefault(); void perform(async () => { try { const created = await request<{ principalId: string }>({ action: "createAccount", request: account }); setPrincipal(created.principalId); await loadAccounts(); setNotice("Compte créé. Transmettez le mot de passe initial par votre canal sécurisé habituel."); } finally { setAccount(a => ({ ...a, password: "" })); } }); }}>
        <h3>Créer un compte</h3><label>Nom<input value={account.displayName} onChange={e => setAccount({ ...account, displayName: e.target.value })} required maxLength={160} /></label><label>Identifiant de connexion<input value={account.login} onChange={e => setAccount({ ...account, login: e.target.value })} required maxLength={254} autoComplete="off" /></label><label>Mot de passe initial, 12 caractères minimum<input type="password" value={account.password} onChange={e => setAccount({ ...account, password: e.target.value })} required minLength={12} maxLength={1024} autoComplete="new-password" /></label><button disabled={busy}>Créer le compte</button>
      </form>
      <h3>Compte de service</h3><label>Nom de l’automatisation<input value={serviceName} onChange={e => setServiceName(e.target.value)} maxLength={160} /></label><label>Nom du jeton<input value={serviceLabel} onChange={e => setServiceLabel(e.target.value)} maxLength={160} /></label><label>Durée de validité en jours<input type="number" min={1} max={90} value={serviceDays} onChange={e => setServiceDays(Number(e.target.value))} /></label>
      <div className="native-buttons">{button("Créer l’automatisation et son jeton", async () => { const value = await request<Data>({ action: "createService", request: { displayName: serviceName, label: serviceLabel, expiresIn: serviceDays * 86400 } }); setPrincipal(String(value.principal_id)); reveal(value, "Jeton de service créé", String(value.principal_id)); await loadAccounts(); }, !serviceName.trim() || !serviceLabel.trim() || serviceDays < 1 || serviceDays > 90)}
      <button type="button" disabled={busy || selectedAccount?.kind !== "service" || !serviceLabel.trim()} onClick={() => confirm("Remplacer le jeton de cette automatisation ? Ses anciens jetons seront immédiatement révoqués.", async () => { const value = await request<Data>({ action: "rotateService", request: { principalId: principal, label: serviceLabel, expiresIn: serviceDays * 86400 } }); reveal(value, "Nouveau jeton de service", principal); })}>Renouveler le jeton du service sélectionné</button></div>
    </details>}

    <details><summary>Accès au projet</summary>
      {button("Charger la gestion des accès", async () => { setPolicy(null); setPolicy(await request<PolicyManagement>({action:"policyManagement"})); })}
      <NativePolicyManagement value={policy} />
      {policyTransport && <NativePolicyEditor scope={scope} management={policy} transport={policyTransport} busy={busy} onBusy={onBusy} onApplied={async () => { setPolicy(null); setPolicy(await request<PolicyManagement>({action:"policyManagement"})); }} />}
      <label>Compte ou service sélectionné<input value={principal} onChange={e => setPrincipal(e.target.value)} placeholder="principal:…" /></label>{selectedAccount && <p>{selectedAccount.displayName}</p>}<label>Accès<select value={role} onChange={e => setRole(e.target.value)}><option value="member">Membre : publier et participer aux revues</option><option value="auditor">Lecture seule</option><option value="release_manager">Gestion des versions : lecture, publication et révocation</option><option value="automation">Automatisation : lecture et preuves CI</option><option value="revoked">Retirer l’accès</option></select></label><button type="button" disabled={busy || !principal || policy?.mode === "custom"} onClick={() => confirm(role === "revoked" ? "Retirer l’accès de ce compte au projet ?" : "Appliquer ces droits à ce compte dans le projet ? Les validations liées à l’ancienne politique deviendront obsolètes.", async () => { const current = await request<PolicyManagement>({action:"policyManagement"}); setPolicy(current); if (current.mode === "custom") throw new Error("Rétablissez la politique gérée avant de modifier les rôles."); await request({ action: "grantMember", request: { principalId: principal, role } }); setNotice("Accès mis à jour."); })}>Appliquer l’accès</button>
    </details>

    <details><summary>Protection des Layers</summary><label>Layer à protéger<input value={layer} onChange={e => { setLayer(e.target.value); setRecipeId(null); }} placeholder="layer:…" /></label>{button("Charger la protection", readProtection, !layer)}<label>Approbations humaines requises<input type="number" value={approvals} min={0} max={10} onChange={e => setApprovals(Number(e.target.value))} /></label><label className="native-check"><input type="checkbox" checked={selfApproval} onChange={e => setSelfApproval(e.target.checked)} />Autoriser l’approbation de l’auteur</label><label className="native-check"><input type="checkbox" checked={rejectConflicts} onChange={e => setRejectConflicts(e.target.checked)} />Refuser les conflits non résolus</label><label className="native-check"><input type="checkbox" checked={requireCI} onChange={e => setRequireCI(e.target.checked)} />Exiger une preuve CI pour la capture exacte</label>
      {requireCI && <><label>Recette JSON exacte<textarea value={recipe} onChange={e => setRecipe(e.target.value)} maxLength={65536} rows={6} placeholder={'{"image":"…@sha256:…","command":["/bin/sh","-c","./ci/test.sh"],"timeoutSeconds":300,"maximumLogBytes":65536}'} /></label>{recipeId && <code>Recette actuelle : {recipeId}</code>}<label>Identifiants des services de confiance, séparés par des virgules<input value={runners} onChange={e => setRunners(e.target.value)} /></label><label>Âge maximal de la preuve en heures<input type="number" min={1} max={720} value={proofHours} onChange={e => setProofHours(Number(e.target.value))} /></label></>}
      {button("Enregistrer la protection", saveProtection, !layer || approvals < 0 || approvals > 10 || (requireCI && (!recipe.trim() || !runners.trim())))}
    </details>

    <details><summary>Webhooks</summary>{button("Charger les webhooks", loadHooks)}<p>Les destinations HTTPS doivent être autorisées dans la configuration de l’instance.</p><label>Adresse HTTPS du récepteur<input type="url" value={hookUrl} onChange={e => setHookUrl(e.target.value)} placeholder="https://outils.exemple/webhooks/layrs" maxLength={2048} /></label><fieldset><legend>Événements à transmettre</legend>{Object.entries(eventNames).map(([kind, label]) => <label key={kind} className="native-check"><input type="checkbox" checked={hookEvents.includes(kind)} onChange={e => setHookEvents(e.target.checked ? [...hookEvents, kind] : hookEvents.filter(event => event !== kind))} />{label}</label>)}</fieldset>
      {button("Créer le webhook", async () => { const value = await request<Data>({ action: "createWebhook", request: { url: hookUrl, eventKinds: hookEvents } }); reveal(value, "Secret de signature du webhook", `${String(value.webhookId)} · génération ${String(value.secretGeneration)}`); await loadHooks(); }, !hookUrl.startsWith("https://") || !hookEvents.length)}
      {hooks.map(hook => <article key={hook.webhookId}><strong>{hook.enabled ? "Actif" : "Désactivé"}</strong><code>{hook.url}</code><code>{hook.webhookId}</code><p>Génération de signature {hook.secretGeneration}</p><div className="native-buttons">{button("Voir les livraisons", () => loadDeliveries(hook.webhookId))}<button type="button" disabled={busy || !hook.enabled} onClick={() => confirm("Renouveler le secret de signature ? Mettez ensuite à jour le récepteur avec la nouvelle génération.", async () => { const value = await request<Data>({ action: "rotateWebhook", webhook: hook.webhookId }); reveal(value, "Nouveau secret de signature", `${hook.webhookId} · génération ${String(value.secretGeneration)}`); await loadHooks(); })}>Renouveler le secret</button><button type="button" disabled={busy || !hook.enabled} onClick={() => confirm("Désactiver les prochaines livraisons de ce webhook ?", async () => { await request({ action: "disableWebhook", webhook: hook.webhookId }); await loadHooks(); })}>Désactiver</button></div></article>)}
      {deliveryHook && <section aria-label="Livraisons du webhook"><h3>Dernières livraisons</h3>{deliveries.map(delivery => <article key={delivery.deliveryId}><code>{delivery.deliveryId}</code><p>{delivery.state === "delivered" ? "Livré" : delivery.state === "dead" ? "Tentatives épuisées" : "En attente"} · {delivery.attempts} tentative(s){delivery.lastStatus ? ` · HTTP ${delivery.lastStatus}` : ""}</p>{delivery.lastError && <small>{delivery.lastError}</small>}{(delivery.state === "dead" || delivery.state === "delivered") && button("Relancer cette livraison", async () => { await request({ action: "replayWebhook", webhook: deliveryHook, deliveryId: delivery.deliveryId }); await loadDeliveries(deliveryHook); })}</article>)}</section>}
    </details>

    <details><summary>Mes appareils et jetons</summary>{button("Charger mes jetons", async () => { const value = await request<{ tokens: Token[] } | Token[]>({ action: "listTokens" }); setTokens(Array.isArray(value) ? value : value.tokens); })}{tokens.map(token => <article key={token.tokenId}><strong>{token.label}</strong><p>{token.kind} · expire le {token.expiresAt}{token.revoked ? " · révoqué" : ""}</p>{!token.revoked && <button type="button" disabled={busy} onClick={() => confirm("Révoquer ce jeton ? Si c’est votre session actuelle, vous devrez vous reconnecter.", async () => { await request({ action: "revokeToken", tokenId: token.tokenId }); setTokens(items => items.map(item => item.tokenId === token.tokenId ? { ...item, revoked: true } : item)); setNotice("Jeton révoqué."); })}>Révoquer</button>}</article>)}</details>
  </details>;
}
