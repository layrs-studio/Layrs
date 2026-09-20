import { useEffect, useId, useRef, useState } from "react";
import type { PolicyManagement } from "./native-policy-management";

type Preview = {
  schemaVersion: "layrs.policy-root-preview.v1" | "layrs.policy-root-preview.v2"; origin: string; principalId: string;
  spaceId: string; expectedPolicySetId: string; expectedAuthorizationEpoch: number;
  proposedPolicySetId: string; request: unknown; inlineRules?: { rules: unknown } | null;
};
export type PolicyTransport = <T>(action: {action: string; [key: string]: unknown}) => Promise<T>;

export function NativePolicyEditor({scope, management, transport, busy, onBusy, onApplied}: {
  scope: string; management: PolicyManagement | null; transport: PolicyTransport;
  busy: boolean; onBusy: (value: boolean) => void; onApplied: () => Promise<void>;
}) {
  const [target, setTarget] = useState("");
  const [rules, setRules] = useState("");
  const rulesId = useId();
  const [preview, setPreview] = useState<Preview | null>(null);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const generation = useRef(0), active = useRef(false);
  useEffect(() => {
    generation.current++; active.current = false; setTarget(""); setRules(""); setPreview(null); setNotice(""); setError("");
    return () => { generation.current++; };
  }, [scope]);
  async function perform(work: (current: number) => Promise<void>) {
    if (busy || active.current) return;
    const current = generation.current;
    active.current = true; onBusy(true); setError(""); setNotice("");
    try { await work(current); }
    catch (e) { if (current === generation.current) setError(String(e)); }
    finally { if (current === generation.current) { active.current = false; onBusy(false); } }
  }
  async function prepare(id: string) {
    await perform(async current => {
      setPreview(null);
      const result = await transport<{preview: Preview}>({action:"previewPolicy",policySetId:id});
      if (current === generation.current) { setTarget(id); setPreview(result.preview); }
    });
  }
  async function apply() {
    if (!preview) return;
    const selected = preview;
    await perform(async current => {
      const result = await transport<{receipt:{status:string;rejections?:{code:string}[]}}>({action:"applyPolicy",preview:selected});
      if (current !== generation.current) return;
      if (result.receipt.status !== "applied") {
        setPreview(null);
        throw new Error(`Modification refusée : ${result.receipt.rejections?.map(r => r.code).join(", ") || result.receipt.status}. Rechargez la politique et préparez un nouvel aperçu.`);
      }
      setPreview(null); setNotice("Politique appliquée. Les droits sont réévalués pour les prochaines actions.");
      // A successful policy change can intentionally remove this user's access.
      try { await onApplied(); }
      catch { if (current === generation.current) setNotice("Politique appliquée ; sa relecture est indisponible. Vérifiez vos nouveaux droits et la connexion."); }
    });
  }
  async function prepareRules() {
    await perform(async current => {
      setPreview(null);
      if (new TextEncoder().encode(rules).length > 64 * 1024) throw new Error("Le document de règles dépasse 64 Kio.");
      let document: unknown;
      try { document = JSON.parse(rules); }
      catch { throw new Error("Le document de règles doit être un JSON valide."); }
      const result = await transport<{preview: Preview}>({action:"previewPolicyRules",rules:document});
      if (current === generation.current) setPreview(result.preview);
    });
  }
  return <details><summary>Changer la politique d’accès</summary>
    <p>Sélectionnez une politique déjà enregistrée sur ce serveur. Son application peut retirer votre propre accès au projet.</p>
    <label>Identifiant de la politique cible<input value={target} disabled={busy} onChange={e => {setTarget(e.target.value); setPreview(null);}} placeholder="blake3:…" /></label>
    <div className="native-buttons">
      <button disabled={busy || !/^blake3:[0-9a-f]{64}$/.test(target.trim())} onClick={() => void prepare(target.trim())}>Préparer le changement de politique</button>
      <button disabled={busy || !management || management.mode !== "custom"} onClick={() => { if (management) void prepare(management.managedPolicySetId); }}>Préparer le retour aux équipes et rôles</button>
    </div>
    <details><summary>Ajouter des règles personnalisées</summary>
      <p>Ajoutez un document de règles d’accès v1 ou v2 aux politiques actuelles. Les refus sont prioritaires. Les équipes et rôles seront suspendus jusqu’au rétablissement de leur politique gérée.</p>
      <label htmlFor={rulesId}>Document JSON des règles</label><textarea id={rulesId} value={rules} disabled={busy} onChange={e => {setRules(e.target.value); setPreview(null);}} />
      <button disabled={busy || !rules.trim()} onClick={() => void prepareRules()}>Préparer l’ajout des règles</button>
    </details>
    {preview && <section aria-label="Aperçu du changement de politique">
      <p>Projet : <code>{preview.spaceId}</code></p>
      <p>Politique actuelle : <code>{preview.expectedPolicySetId}</code></p>
      <p>Politique proposée : <code>{preview.proposedPolicySetId}</code></p>
      <p>Révision des droits attendue : {preview.expectedAuthorizationEpoch}. Une modification concurrente sera refusée.</p>
      {preview.inlineRules && <details open><summary>Règles ajoutées aux politiques actuelles</summary><pre>{JSON.stringify(preview.inlineRules.rules, null, 2)}</pre></details>}
      <p>En cas de réponse perdue, réessayez ce même aperçu pour connaître le résultat.</p>
      <button disabled={busy} onClick={() => void apply()}>Appliquer cet aperçu de politique</button>
    </section>}
    {notice && <p role="status">{notice}</p>}{error && <p role="alert">{error}</p>}
  </details>;
}
