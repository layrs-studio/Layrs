import { useEffect, useId, useRef, useState } from "react";
import type { PolicyManagement } from "./native-policy-management";
import { NativePolicyPathForm } from "./native-policy-path-form";
import type { NativeAdminCommand } from "./native-admin-route";
import type { NativePolicyRulesRead } from "./native-policy-path-form-model";

type Preview = {
  schemaVersion: "layrs.policy-root-preview.v1" | "layrs.policy-root-preview.v2" | "layrs.policy-root-preview.v3"; origin: string; principalId: string;
  spaceId: string; expectedPolicySetId: string; expectedAuthorizationEpoch: number;
  proposedPolicySetId: string; request: unknown; inlineRules?: { rules: unknown; replacement?:{policyId:string;previousProductObjectId:string} } | null;
};
export type PolicyTransport = <T>(action: {action: string; [key: string]: unknown}) => Promise<T>;

export function NativePolicyEditor({scope, management, transport, administration, busy, onBusy, onApplied}: {
  scope: string; management: PolicyManagement | null; transport: PolicyTransport;
  busy: boolean; onBusy: (value: boolean) => void; onApplied: () => Promise<void>;
  administration?: <T>(command:NativeAdminCommand)=>Promise<T>;
}) {
  const [target, setTarget] = useState("");
  const [rules, setRules] = useState("");
  const [rulesPolicy,setRulesPolicy]=useState("");
  const [readPolicies,setReadPolicies]=useState<NativePolicyRulesRead|null>(null);
  const rulesId = useId();
  const [preview, setPreview] = useState<Preview | null>(null);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const generation = useRef(0), active = useRef(false);
  useEffect(() => {
    generation.current++; active.current = false; setTarget(""); setRules("");setRulesPolicy("");setReadPolicies(null); setPreview(null); setNotice(""); setError("");
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
      const result = await transport<{preview: Preview}>({action:rulesPolicy?"previewReplacePolicyRules":"previewPolicyRules",...(rulesPolicy?{policyId:rulesPolicy}:{}),rules:document});
      if(rulesPolicy && (!readPolicies || result.preview.expectedPolicySetId!==readPolicies.policySetId || result.preview.expectedAuthorizationEpoch!==readPolicies.authorizationEpoch))throw new Error("La politique a changé. Relisez ses règles avant de préparer le remplacement.");
      if (current === generation.current) setPreview(result.preview);
    });
  }
  return <details><summary>Changer la politique d’accès</summary>
    {administration&&<NativePolicyPathForm scope={scope} transport={transport} administration={administration} busy={busy} onBusy={onBusy} onInvalidate={()=>setPreview(null)} onPreview={p=>{setPreview(p as Preview);setError("");setNotice("");}} />}
    <p>Sélectionnez une politique déjà enregistrée sur ce serveur. Son application peut retirer votre propre accès au projet.</p>
    <label>Identifiant de la politique cible<input value={target} disabled={busy} onChange={e => {setTarget(e.target.value); setPreview(null);}} placeholder="blake3:…" /></label>
    <div className="native-buttons">
      <button disabled={busy || !/^blake3:[0-9a-f]{64}$/.test(target.trim())} onClick={() => void prepare(target.trim())}>Préparer le changement de politique</button>
      <button disabled={busy || !management || management.mode !== "custom"} onClick={() => { if (management) void prepare(management.managedPolicySetId); }}>Préparer le retour aux équipes et rôles</button>
    </div>
    <details><summary>Ajouter des règles personnalisées</summary>
      <p>Ajoutez un document de règles d’accès v1 ou v2 aux politiques actuelles. Les refus sont prioritaires. Les équipes et rôles seront suspendus jusqu’au rétablissement de leur politique gérée.</p>
      <button disabled={busy} onClick={()=>void perform(async current=>{const snapshot=await transport<NativePolicyRulesRead>({action:"readPolicyRules"});if(current===generation.current){setReadPolicies(snapshot);setRulesPolicy("");setPreview(null);}})}>Lire les politiques actuelles pour remplacement</button>
      {readPolicies&&<label>Politique du document JSON<select disabled={busy} value={rulesPolicy} onChange={e=>{const selected=readPolicies.policies.find(p=>p.policyId===e.target.value);setRulesPolicy(e.target.value);setRules(selected?JSON.stringify(selected.rules,null,2):"");setPreview(null);}}><option value="">Ajouter une nouvelle politique indépendante</option>{readPolicies.policies.map(p=><option key={p.policyId} value={p.policyId}>{p.policyId}</option>)}</select></label>}
      <label htmlFor={rulesId}>Document JSON des règles</label><textarea id={rulesId} value={rules} disabled={busy} onChange={e => {setRules(e.target.value); setPreview(null);}} />
      {rulesPolicy&&<p>Le document remplace entièrement les règles de cette seule politique. Son identité et les autres politiques sont conservées ; son ancienne version reste dans l’historique.</p>}
      <button disabled={busy || !rules.trim()} onClick={() => void prepareRules()}>{rulesPolicy?"Préparer le remplacement des règles":"Préparer l’ajout des règles"}</button>
    </details>
    {preview && <section aria-label="Aperçu du changement de politique">
      <p>Projet : <code>{preview.spaceId}</code></p>
      <p>Politique actuelle : <code>{preview.expectedPolicySetId}</code></p>
      <p>Politique proposée : <code>{preview.proposedPolicySetId}</code></p>
      <p>Révision des droits attendue : {preview.expectedAuthorizationEpoch}. Une modification concurrente sera refusée.</p>
      {preview.inlineRules?.replacement&&<p>Politique remplacée : <code>{preview.inlineRules.replacement.policyId}</code> · Version précédente : <code>{preview.inlineRules.replacement.previousProductObjectId}</code>. Les autres politiques restent identiques.</p>}
      {preview.inlineRules && <details open><summary>{preview.inlineRules.replacement?"Nouvelles règles de la politique sélectionnée":"Règles ajoutées aux politiques actuelles"}</summary><pre>{JSON.stringify(preview.inlineRules.rules, null, 2)}</pre></details>}
      <p>En cas de réponse perdue, réessayez ce même aperçu pour connaître le résultat.</p>
      <button disabled={busy} onClick={() => void apply()}>Appliquer cet aperçu de politique</button>
    </section>}
    {notice && <p role="status">{notice}</p>}{error && <p role="alert">{error}</p>}
  </details>;
}
