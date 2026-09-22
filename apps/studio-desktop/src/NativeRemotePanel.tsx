import { NativeAdminSurface, NativeRemoteProjectFiles } from "@layrs/ui";
import { useEffect, useRef, useState } from "react";
import { nativeRemote } from "./tauri";
import { NativeReviewPanel } from "./NativeReviewPanel";
import { NativeChoosePanel } from "./NativeChoosePanel";

type Reply = { status?: string; principalId?: string; principal_id?: string; userCode?: string; verificationUri?: string; interval?: number; receipt?: {status:string}; received?: { divergent: boolean }; state?: { worktree: { checkoutRoot: string } } };
export function NativeRemotePanel({ root, localSpaceId, opened, busy, onBusy, onChanged }: { root: string; localSpaceId?: string; opened: boolean; busy: boolean; onBusy: (value: boolean) => void; onChanged: (root: string) => void }) {
  const [origin, setOrigin] = useState(() => localStorage.getItem("layrs.native.origin") || "");
  const [ca, setCa] = useState("");
  const [workspace, setWorkspace] = useState("");
  const [slug, setSlug] = useState("");
  const [space, setSpace] = useState("");
  const [projectionStep,setProjectionStep]=useState("");
  const [migrationDestination,setMigrationDestination]=useState(""),[migrationLayer,setMigrationLayer]=useState("");
  const [reply, setReply] = useState<Reply | null>(null);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [sessionEpoch, setSessionEpoch] = useState(0);
  const [principal, setPrincipal] = useState("");
  const [selectedReview, setSelectedReview] = useState<{reviewId:string;version:number} | null>(null);
  const [transferId,setTransferId]=useState<string|null>(null);
  const [cancelling,setCancelling]=useState(false);
  const activeTransfer=useRef<{id:string;root:string;origin:string;ca:string;publication:boolean}|null>(null);
  const inFlight=useRef(false);
  useEffect(() => setSelectedReview(null), [root,origin,principal]);
  async function cancelTransfer() {
    const current=activeTransfer.current;
    if(!current || cancelling)return;
    setCancelling(true);
    try {
      const result=await nativeRemote<{status:string}>(current.root,{origin:current.origin,caPem:current.ca||null,action:{action:"cancelTransfer",transferId:current.id}});
      if(activeTransfer.current?.id!==current.id)return;
      setMessage(result.status==="tooLate" ? current.publication ? "La publication a déjà été engagée ; attendez son reçu." : "L’application locale a déjà commencé ; attendez son résultat." : result.status==="notFound" ? "Annulation indisponible pour le moment. Réessayez si le transfert reste actif." : "Arrêt demandé. Le résultat du transfert précisera si l’annulation a pu être effectuée.");
    }catch(e){if(activeTransfer.current?.id===current.id)setError(String(e));}
    finally{if(activeTransfer.current?.id===current.id)setCancelling(false);}
  }
  async function run(action: { action: string; [key: string]: unknown }) {
    if (busy || inFlight.current) return;
    inFlight.current=true;
    onBusy(true); setError(""); setMessage("");
    const cancellable=["clone","receive","publish","publishCandidates"].includes(action.action);
    const id=cancellable ? crypto.randomUUID() : null;
    if(id){activeTransfer.current={id,root,origin,ca,publication:action.action.startsWith("publish")};setTransferId(id);setCancelling(false);}
    let refresh: string | undefined;
    try {
      const request=id ? {...action,action:`${action.action}Cancellable`,transferId:id} : action;
      const next = await nativeRemote<Reply>(root, { origin, caPem: ca || null, action:request });
      if(next.receipt && next.receipt.status!=="applied")throw new Error("Publication refusée. Recevez la vue actuelle, puis vérifiez les droits, les protections et les têtes avant un nouvel essai.");
      if (next.status === "authorized" || action.action === "whoami") setPrincipal(next.principalId || next.principal_id || "");
      if (next.status === "signedOut" || action.action === "startLogin") setPrincipal("");
      setReply(previous => next.status === "pending" ? { ...previous, ...next } : next); localStorage.setItem("layrs.native.origin", origin);
      if (next.status === "authorized" || next.status === "signedOut") { setSessionEpoch(value => value + 1); setSelectedReview(null); }
      setMessage(next.status === "pending" ? "Validez la connexion dans votre navigateur, puis vérifiez ici." : next.status === "authorized" ? "Connexion autorisée. Le jeton est conservé dans le coffre du système." : next.status === "signedOut" ? "Session révoquée." : next.received?.divergent ? "Réception terminée. Plusieurs têtes sont conservées ; une intégration reste nécessaire." : "Opération terminée.");
      if(action.action==="publishCandidates")setMessage("Captures candidates admises. Les têtes distantes restent inchangées ; prévisualisez maintenant votre choix.");
      if (["clone", "cloneProjection", "receive", "publish", "provision"].includes(action.action)) refresh = next.state?.worktree.checkoutRoot || root;
      if(action.action==="migrateProjection")refresh=String(action.destination);
    } catch (e) {
      const detail=String(e);
      if(/remote_unauthorized|Sign in to this server first/.test(detail)){setPrincipal("");setSessionEpoch(value=>value+1);setSelectedReview(null);}
      if(detail.includes("remote_transfer_cancelled"))setMessage(action.action.startsWith("publish") ? "Publication annulée avant envoi de la transaction. Les têtes distantes, les fichiers et l’historique local sont conservés." : "Transfert annulé avant application. Les fichiers et l’historique local sont conservés.");
      else if(detail.includes("publication_pending"))setError("Une publication précédente attend son reçu. Rejouez la publication pour connaître son résultat ; son annulation ne peut pas être confirmée.");
      else if(detail.includes("content_access_restricted"))setError("Votre accès est limité à certains fichiers. Le clonage et la réception complète sont indisponibles avec ces droits. Vous pouvez consulter l’historique et télécharger un fichier autorisé avec la CLI. Vos fichiers locaux sont conservés.");
      else if(cancellable && /projection_dirty|capture or preserve edits/.test(detail))setError("Capturez vos modifications avant ce transfert annulable.");
      else setError(detail);
    }
    finally { activeTransfer.current=null;setTransferId(null);setCancelling(false);inFlight.current=false;onBusy(false); }
    if (refresh) onChanged(refresh);
  }
  return <details className="native-remote"><summary>Serveur et synchronisation</summary>
    <div className="native-open">
      <label>Serveur HTTPS<input type="url" placeholder="https://layrs.monentreprise.fr" value={origin} disabled={busy} onChange={e => { setOrigin(e.target.value); setReply(null); setPrincipal(""); setSessionEpoch(value => value + 1); setSelectedReview(null); }} /></label>
      <button disabled={busy || !origin} onClick={() => void run({ action: "startLogin" })}>Se connecter</button>
      <button disabled={busy || !origin} onClick={() => void run({ action: "whoami" })}>Vérifier la session</button>
      <button disabled={busy || !origin} onClick={() => void run({ action: "logout" })}>Se déconnecter</button>
    </div>
    <details><summary>Certificat privé de l’entreprise</summary><label>Autorité de certification PEM<textarea value={ca} disabled={busy} onChange={e => setCa(e.target.value)} placeholder="Facultatif si le certificat du serveur est reconnu par le système." /></label></details>
    {reply?.status === "pending" && <p>Ouvrez <a href={reply.verificationUri} target="_blank" rel="noreferrer">la page de connexion</a> et confirmez le code <strong>{reply.userCode}</strong>. <button disabled={busy} onClick={() => void run({ action: "pollLogin" })}>Vérifier la connexion</button></p>}
    {opened ? <>
      <div className="native-open"><label>Organisation<input value={workspace} disabled={busy} onChange={e => setWorkspace(e.target.value)} /></label><label>Identifiant du projet<input value={slug} disabled={busy} onChange={e => setSlug(e.target.value)} /></label><button disabled={busy || !origin || !workspace || !slug} onClick={() => void run({ action: "provision", workspace, slug })}>Créer le projet sur le serveur</button></div>
      <div className="native-toolbar"><button disabled={busy || !origin} onClick={() => void run({ action: "publish", integrate: false })}>Publier</button><button disabled={busy || !origin} onClick={() => void run({ action: "receive" })}>Recevoir</button><button disabled={busy || !origin} onClick={() => void run({ action: "publish", integrate: true })}>Publier en conservant les têtes concurrentes</button></div>
    </> : <div className="native-open"><label>Identifiant du projet distant<input value={space} disabled={busy} onChange={e => setSpace(e.target.value)} placeholder="space:…" /></label><button disabled={busy || !origin || !root || !space} onClick={() => void run({ action: "clone", space, name: space })}>Cloner dans le dossier vide sélectionné</button></div>}
    {message && <p role="status">{message}</p>}{error && <p className="native-error" role="alert">{error}</p>}
    {!opened&&<details><summary>Cloner explicitement une projection partielle</summary><p>Le dossier doit être vide. Seuls les fichiers autorisés sont matérialisés ; les chemins interdits restent réservés et visibles.</p><label>Capture publiée à projeter<input value={projectionStep} disabled={busy} onChange={e=>setProjectionStep(e.target.value)} placeholder="blake3:…"/></label><button disabled={busy||!origin||!root||!space||!/^blake3:[a-f0-9]{64}$/.test(projectionStep)} onClick={()=>void run({action:"cloneProjection",space,step:projectionStep})}>Cloner les fichiers autorisés</button></details>}
    {opened&&<details><summary>Passer à une projection partielle</summary><p>Le projet doit être publié et sans modifications locales. Les fichiers autorisés seront copiés dans un autre dossier vide ; le dossier actuel et son historique seront conservés.</p><label>Dossier vide de destination<input value={migrationDestination} disabled={busy} onChange={e=>setMigrationDestination(e.target.value)}/></label><label>Layer publié à projeter<input value={migrationLayer} disabled={busy} onChange={e=>setMigrationLayer(e.target.value)} placeholder="layer:…"/></label><button disabled={busy||!origin||!migrationDestination.trim()||!migrationLayer.trim()} onClick={()=>void run({action:"migrateProjection",destination:migrationDestination.trim(),layerId:migrationLayer.trim()})}>Créer la projection et conserver ce dossier</button></details>}
    {transferId && <p role="status">Transfert en cours. <button disabled={cancelling} onClick={()=>void cancelTransfer()}>Annuler le transfert</button></p>}
    {opened && principal && <NativeRemoteProjectFiles key={`files:${sessionEpoch}:${origin}:${root}:${principal}`} scope={`${sessionEpoch}:${origin}:${root}:${principal}:${ca}`} busy={busy || !origin} onBusy={onBusy}
      loadContext={()=>nativeRemote(root,{origin,caPem:ca||null,action:{action:"collaboration",command:{action:"projectionContext"}}})}
      loadPage={request=>nativeRemote(root,{origin,caPem:ca||null,action:{action:"collaboration",command:{action:"projectionPage",...request}}})} />}
    {opened && principal && <NativeChoosePanel key={`choose:${sessionEpoch}:${origin}:${root}:${principal}`} root={root} origin={origin} ca={ca} busy={busy} onBusy={onBusy} onPublishCandidates={()=>void run({action:"publishCandidates"})} />}
    <NativeAdminSurface onOpenReview={setSelectedReview} key={`admin:${sessionEpoch}:${origin}:${root}:${principal}`} scope={`${sessionEpoch}:${origin}:${root}:${principal}`} busy={busy} onBusy={onBusy} transport={async <T,>(command: { action: string; [key: string]: unknown }) => nativeRemote<T>(root, { origin, caPem: ca || null, action: { action: "administration", command } })} policyTransport={async <T,>(action: {action: string; [key: string]: unknown}) => nativeRemote<T>(root, {origin,caPem:ca || null,action})} />
    {opened && <NativeReviewPanel key={`${sessionEpoch}:${principal}`} root={root} spaceId={localSpaceId} origin={origin} principal={principal} selectedReview={selectedReview} ca={ca} busy={busy} onBusy={onBusy} />}
  </details>;
}
