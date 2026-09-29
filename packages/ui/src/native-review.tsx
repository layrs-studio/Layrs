import { useEffect, useRef, useState } from "react";
import { NativeCompareSurface } from "./native-compare";
import { useReviewCommentDraft } from "./native-review-draft-form";
import type { ReviewDraftIdentity } from "./native-review-drafts";
import { exactReviewVersion, type NativeReviewContext } from "./native-review-context";
import { NativeReviewComments, type CommentCorrectionRequest } from "./native-review-comments";
import type { CommentCorrection, ReviewCommentAction } from "./native-review-comment-history";
import { useReviewProposalDraft } from "./native-review-proposal-draft-form";
import { nativeRunPresentation } from "./native-run-state";

type Proposal = { spaceId: string; reviewId: string; version: number; title: string; proposedStepId: string; layerId: string; authorPrincipalId: string; participants: string[] };
type Review = { lifecycle: string; proposal: Proposal; decisionState?: { state: string; approvals: number; requiredApprovals: number; blockingDecisions: number; stale: boolean } };
type Detail = { lifecycle: string; integration?: { evidenceObjectId: string; record: { version: number; transactionId: string; integratedAt: string; integratedBy: string } } | null; versions: Proposal[]; actions: ReviewCommentAction[]; commentCorrections?: CommentCorrection[] };
type Run = { runId: string; stepId: string; outcome: string | null; proofObjectId: string | null; expiresAt?: string; status?: "running" | "expired" | "completed" | "cancelled"; cancellation?: { evidenceObjectId: string; record: { cancelledBy: string; cancelledAt: string; reason: string } } | null };
type RunDetail = Run & { runnerPrincipalId: string; recipeObjectId: string; evidence: { schemaVersion: string; log?: string }[] };
export function NativeReviewSurface({ scope, spaceId, draftIdentity, selectedReview, transport, busy, onBusy }: { scope: string; spaceId?: string; draftIdentity?: ReviewDraftIdentity; selectedReview?: NativeReviewContext | null; transport: <T>(command: { action: string; [key: string]: unknown }) => Promise<T>; busy: boolean; onBusy: (busy: boolean) => void }) {
  const [reviews, setReviews] = useState<Review[]>([]);
  const [reviewsCursor, setReviewsCursor] = useState<string | null>(null);
  const [detail, setDetail] = useState<Detail | null>(null);
  const [requestedVersion, setRequestedVersion] = useState<number | null>(null);
  const surface = useRef<HTMLDetailsElement>(null);
  const selectionAttempt = useRef<{ session: string; target: NativeReviewContext } | null>(null);
  const [proposalMode, setProposalMode] = useState("new");
  const [runs, setRuns] = useState<Run[]>([]);
  const [runsCursor, setRunsCursor] = useState<string | null>(null);
  const [runDetail, setRunDetail] = useState<RunDetail | null>(null);
  const [cancellingRun, setCancellingRun] = useState<Run | null>(null);
  const [cancellationReason, setCancellationReason] = useState("");
  const [error, setError] = useState("");
  const pending = useRef({ signature: "", key: "" });
  const generation = useRef(0);
  const inFlight = useRef(false);
  const currentProposal = detail?.versions.at(-1);
  const latest = requestedVersion === null ? currentProposal : detail?.versions.find(version => version.version === requestedVersion);
  const historical = Boolean(latest && currentProposal && latest.version !== currentProposal.version);
  const session = JSON.stringify([scope, draftIdentity?.origin, draftIdentity?.principalId]);
  const proposalTarget = proposalMode === "new" ? { reviewId: null, expectedVersion: 0 }
    : latest ? { reviewId: latest.reviewId, expectedVersion: proposalMode.startsWith("resume:") ? Number(proposalMode.slice(7)) : latest.version } : null;
  const proposalDraft = useReviewProposalDraft(session, draftIdentity, spaceId, proposalTarget, latest?.reviewId);
  const proposalInput = proposalDraft.input;
  const resumingProposal = proposalMode.startsWith("resume:");
  const proposalWritable = proposalMode === "new" || (!historical && Boolean(latest) && (resumingProposal || detail?.lifecycle === "open"));
  const draft = useReviewCommentDraft(session, draftIdentity, latest);
  const { body, path, line, replyTo, setBody, setPath, setLine, setReplyTo } = draft;
  useEffect(() => { generation.current++; setReviews([]); setReviewsCursor(null); setDetail(null); setRequestedVersion(null); setProposalMode("new"); setRuns([]); setRunsCursor(null); setRunDetail(null); setCancellingRun(null); setCancellationReason(""); setError(""); pending.current = { signature: "", key: "" }; return () => { generation.current++; }; }, [session]);
  useEffect(() => {
    if (!selectedReview || busy || inFlight.current || (selectionAttempt.current?.session === session && selectionAttempt.current.target === selectedReview)) return;
    selectionAttempt.current = { session, target: selectedReview };
    if (surface.current) { surface.current.open = true; surface.current.scrollIntoView({ block: "start", behavior: "smooth" }); }
    void perform(() => load(selectedReview.reviewId, selectedReview.version));
  }, [session, selectedReview, busy]);
  async function request<T>(command: { action: string; [key: string]: unknown }): Promise<T> {
    const current=generation.current;
    const result=await transport<T>(command);
    if(current!==generation.current)throw new Error("La session a changé.");
    return result;
  }
  function keyed(value: Record<string, unknown>, scope = "") {
    const signature = JSON.stringify({ scope, value });
    if (signature !== pending.current.signature) pending.current = { signature, key: crypto.randomUUID() };
    return { ...value, idempotencyKey: pending.current.key };
  }
  async function perform(work: () => Promise<void>) {
    if (busy || inFlight.current) return;
    inFlight.current=true;
    onBusy(true); setError("");
    const current = generation.current;
    try { await work(); } catch (e) { if (current === generation.current) { setReviews([]); setReviewsCursor(null); setDetail(null); setRuns([]); setRunsCursor(null); setRunDetail(null); setError(String(e)); } }
    finally { inFlight.current=false; onBusy(false); }
  }
  async function load(id: string, version: number | null = null) {
    const loaded = await request<Detail>({ action: "review", review: id });
    if (version !== null) exactReviewVersion(loaded.versions, { reviewId: id, version });
    setDetail(loaded); setRequestedVersion(version);
  }
  async function loadReviews(next = false) {
    const result = await request<{ reviews: Review[]; nextCursor: string | null }>({ action: "listReviewsPage", cursor: next ? reviewsCursor : null, limit: 50 });
    setReviews(result.reviews); setReviewsCursor(result.nextCursor);
  }
  async function loadRuns(next = false) {
    const result = await request<{runs: Run[]; nextCursor: string | null}>({action:"listRunsPage",cursor:next ? runsCursor : null,limit:50});
    setRuns(result.runs); setRunsCursor(result.nextCursor); setRunDetail(null);
  }
  async function cancelSelectedRun() {
    if (!cancellingRun) return;
    const result = await request<Pick<Run, "status" | "cancellation">>({ action: "cancelRun", run: cancellingRun.runId, request: keyed({ reason: cancellationReason }, `cancel:${cancellingRun.runId}`) });
    setRuns(items => items.map(run => run.runId === cancellingRun.runId ? { ...run, ...result } : run));
    setRunDetail(current => current?.runId === cancellingRun.runId ? { ...current, ...result } : current);
    pending.current = { signature: "", key: "" }; setCancellingRun(null); setCancellationReason("");
  }
  async function propose() {
    if (busy || inFlight.current || !proposalWritable) return;
    const current = generation.current;
    inFlight.current = true; onBusy(true); setError("");
    try {
      const sent = proposalDraft.submission();
      const proposal = await request<Proposal>({ action: "propose", review: sent.target.reviewId, request: sent.request });
      proposalDraft.clearAfterSuccess(sent); setProposalMode("new");
      await load(proposal.reviewId); await loadReviews();
    } catch (error) { if (current === generation.current) setError(String(error)); }
    finally { inFlight.current = false; onBusy(false); }
  }
  async function decide(action: string) {
    if (!latest || historical) return;
    const sent = draft.submission();
    const input = sent.input;
    const payload = { version: latest.version, action, body: input.body, anchor: input.path ? { path: input.path, startLine: input.line, endLine: input.line } : null, replyTo: action === "comment" ? input.replyTo : null };
    const requestPayload = action === "comment" && sent.draftKey ? { ...payload, idempotencyKey: sent.draftKey } : keyed(payload, latest.reviewId);
    await request({ action: "decide", review: latest.reviewId, request: requestPayload });
    pending.current = { signature: "", key: "" }; draft.clearAfterSuccess(sent); await load(latest.reviewId);
  }
  async function correctComment(comment: string, payload: CommentCorrectionRequest) {
    if (!latest || busy || inFlight.current) throw new Error("Une autre action est en cours.");
    inFlight.current = true; onBusy(true);
    try {
      await request({ action: "correctComment", review: latest.reviewId, comment, request: payload });
      await load(latest.reviewId, requestedVersion);
    } finally { inFlight.current = false; onBusy(false); }
  }
  return <details ref={surface}><summary>Revues et validations</summary>
    <NativeCompareSurface key={`${scope}:${latest?.reviewId || ""}:${latest?.version || 0}`} transport={transport} busy={busy} onBusy={onBusy} after={latest?.proposedStepId} />
    <div className="native-buttons"><button disabled={busy} onClick={() => void perform(() => loadReviews())}>Actualiser les revues</button><button disabled={busy || !reviewsCursor} onClick={() => void perform(() => loadReviews(true))}>Revues suivantes</button><button disabled={busy} onClick={() => void perform(() => loadRuns())}>Consulter les exécutions CI</button><button disabled={busy || !runsCursor} onClick={() => void perform(() => loadRuns(true))}>Exécutions suivantes</button></div>
    {reviews.map(review => <article key={review.proposal.reviewId}><strong>{review.proposal.title}</strong><p>Version {review.proposal.version} · {review.lifecycle === "integrated" ? "Intégrée" : review.lifecycle === "closed" ? "Fermée" : "Ouverte"}</p>{review.decisionState && <p>{review.decisionState.stale ? "Version à actualiser" : review.decisionState.state === "changes_requested" ? "Modifications demandées" : review.decisionState.state === "approved" ? "Approbations réunies" : "En attente de validation"} · {review.decisionState.approvals}/{review.decisionState.requiredApprovals} approbations. Les droits et preuves seront revérifiés à l’intégration.</p>}<button disabled={busy} onClick={() => void perform(() => load(review.proposal.reviewId))}>Ouvrir la revue</button></article>)}
    <details><summary>Proposer une capture publiée</summary><p>Publiez d’abord le Layer contenant la capture proposée.</p>
      <label>Cible de la proposition<select disabled={busy} value={proposalMode} onChange={event => setProposalMode(event.target.value)}><option value="new">Nouvelle revue</option>{latest && <option value="update">Nouvelle version de la revue ouverte · version {latest.version}</option>}{proposalDraft.targets.filter(target => latest && target.expectedVersion < latest.version).map(target => <option key={target.expectedVersion} value={`resume:${target.expectedVersion}`}>Reprendre le brouillon ciblant la version {target.expectedVersion}</option>)}</select></label>
      {resumingProposal && <p>Reprise avec la version attendue et la clé d’envoi d’origine. Le serveur reconnaîtra un envoi déjà accepté ou refusera une proposition devenue obsolète, sans la déplacer sur une nouvelle version.</p>}
      {proposalDraft.available && <aside aria-label="Brouillon de proposition disponible"><p>Une proposition est conservée pour ce compte, ce projet et cette cible exacte.</p><button disabled={busy} onClick={proposalDraft.restore}>Restaurer la proposition</button><button disabled={busy} onClick={proposalDraft.discard}>Supprimer le brouillon de proposition</button></aside>}
      {proposalDraft.notice && <p role="status">{proposalDraft.notice}</p>}
      {!proposalDraft.persistent && <p>La proposition reste en mémoire : une identité vérifiée et le projet exact sont nécessaires pour la conserver sur cet appareil.</p>}
      {proposalDraft.persistent && <p>Brouillon local récupérable pendant 30 jours ; restauration et publication restent explicites.</p>}
      {proposalDraft.key && !proposalDraft.available && <button disabled={busy} onClick={proposalDraft.discard}>Supprimer le brouillon de proposition</button>}
      <label>Titre<input disabled={busy || !proposalDraft.ready} maxLength={1024} value={proposalInput.title} onChange={event => proposalDraft.change({ title: event.target.value })} /></label><label>Layer cible (identifiant)<input disabled={busy || !proposalDraft.ready} maxLength={256} value={proposalInput.layerId} onChange={event => proposalDraft.change({ layerId: event.target.value })} placeholder="layer:…" /></label><label>Capture proposée<input disabled={busy || !proposalDraft.ready} maxLength={256} value={proposalInput.stepId} onChange={event => proposalDraft.change({ stepId: event.target.value })} placeholder="blake3:…" /></label><label>Participants (identifiants séparés par des virgules)<input disabled={busy || !proposalDraft.ready} maxLength={32768} value={proposalInput.participants} onChange={event => proposalDraft.change({ participants: event.target.value })} placeholder="Facultatif" /></label>
      <button disabled={busy || !proposalDraft.ready || !proposalWritable || !proposalInput.title.trim() || !proposalInput.layerId || !proposalInput.stepId} onClick={() => void propose()}>{proposalMode === "new" ? "Créer la revue" : resumingProposal ? "Reprendre l’envoi de la proposition" : "Créer une nouvelle version de la revue ouverte"}</button>
    </details>
    {latest && <section aria-label="Revue ouverte"><h3>{latest.title} · version {latest.version}</h3><code>{latest.proposedStepId}</code><p>Les décisions concernent cette version exacte. Une nouvelle proposition doit être approuvée à nouveau.</p>
      {historical && <aside aria-label="Version historique"><p>Version historique ouverte depuis sa notification, en lecture seule. Ses commentaires et chemins conservent leur contexte d’origine.</p><button disabled={busy} onClick={() => void perform(() => load(latest.reviewId))}>Ouvrir la version actuelle</button></aside>}
      {detail?.integration?.record.version === latest.version && <><p>Intégration enregistrée le {detail.integration.record.integratedAt}.</p><details><summary>Preuve d’intégration</summary><code>{detail.integration.evidenceObjectId}</code><code>{detail.integration.record.transactionId}</code></details></>}
      {detail && <NativeReviewComments key={`${session}:${latest.reviewId}:${latest.version}`} reviewId={latest.reviewId} version={latest.version} historical={historical} actions={detail.actions} corrections={detail.commentCorrections ?? []} principalId={draftIdentity?.principalId} busy={busy} canReply={draft.ready} onReply={setReplyTo} onCorrect={correctComment} />}
      {replyTo && <p>Réponse au commentaire sélectionné. <button disabled={busy || !draft.ready} onClick={() => setReplyTo(null)}>Annuler la réponse</button></p>}
      {draft.available && <aside aria-label="Brouillon local disponible"><p>Un brouillon est conservé pour ce compte et cette version exacte.</p><button disabled={busy} onClick={draft.restore}>Restaurer le brouillon</button><button disabled={busy} onClick={draft.discard}>Supprimer le brouillon local</button></aside>}
      {draft.notice && <p role="status">{draft.notice}</p>}
      {!draft.persistent && <p>Vérifiez votre session pour conserver un brouillon sur cet appareil. La saisie reste en mémoire.</p>}
      {draft.persistent && <p>Brouillons récupérables sur cet appareil pendant 30 jours, puis supprimés à la prochaine sauvegarde. Aucun envoi automatique.</p>}
      {draft.persistedKey && !draft.available && <button disabled={busy} onClick={draft.discard}>Supprimer le brouillon local</button>}
      <p>Vous pouvez répondre à un commentaire ou corriger votre propre texte avec un motif. L’original et ses révisions restent consultables.</p>
      <label>Commentaire<textarea disabled={busy || historical || !draft.ready} value={body} onChange={e => setBody(e.target.value)} maxLength={65536} /></label><div className="native-open"><label>Chemin exact, facultatif<input disabled={busy || historical || !draft.ready} maxLength={4096} value={path} onChange={e => setPath(e.target.value)} /></label><label>Ligne<input disabled={busy || historical || !draft.ready} type="number" value={line} min={1} max={2000000} onChange={e => setLine(Number(e.target.value))} /></label></div>
      <div className="native-buttons">{[["comment", "Commenter"], ["approve", "Approuver cette version"], ["request_changes", "Demander des modifications"], [detail?.lifecycle === "closed" ? "reopen" : "close", detail?.lifecycle === "closed" ? "Rouvrir" : "Fermer"]].map(([action, label]) => <button key={action} disabled={busy || historical || !draft.ready || (action === "comment" && !body.trim()) || (detail?.lifecycle === "integrated" && action !== "comment")} onClick={() => void perform(() => decide(action))}>{label}</button>)}</div>
    </section>}
    {runs.map(run => { const presentation = nativeRunPresentation(run); return <article key={run.runId}><strong>{presentation.label}</strong><p>{presentation.detail}</p><code>{run.stepId}</code>{run.proofObjectId && <code>{run.proofObjectId}</code>}<button disabled={busy} onClick={() => void perform(async () => setRunDetail(await request<RunDetail>({ action: "runDetail", run: run.runId })))}>Consulter la preuve et le journal</button>{presentation.canCancel && <button disabled={busy} onClick={() => { setCancellingRun(run); setCancellationReason(""); }}>Annuler cette exécution</button>}</article>; })}
    {cancellingRun && <section aria-label="Annulation d’une exécution"><h3>Annuler la publication du résultat</h3><code>{cancellingRun.runId}</code><p>Le serveur refusera toute preuve ultérieure de cette exécution. Cette action ne termine pas un conteneur distant : son délai maximal reste applicable.</p><label>Motif d’annulation<textarea disabled={busy} maxLength={4096} value={cancellationReason} onChange={event => setCancellationReason(event.target.value)} /></label><button disabled={busy || !cancellationReason.trim()} onClick={() => void perform(cancelSelectedRun)}>Confirmer l’annulation</button><button disabled={busy} onClick={() => setCancellingRun(null)}>Conserver l’exécution</button></section>}
    {runDetail && <section aria-label="Preuve et journal CI"><h3>Résultat de la capture exacte</h3><code>{runDetail.stepId}</code><p>Exécuteur : {runDetail.runnerPrincipalId}</p><p>Recette : <code>{runDetail.recipeObjectId}</code></p>{runDetail.cancellation && <aside><p>Annulé le {runDetail.cancellation.record.cancelledAt} par {runDetail.cancellation.record.cancelledBy}.</p><p style={{whiteSpace:"pre-wrap"}}>{runDetail.cancellation.record.reason}</p><code>{runDetail.cancellation.evidenceObjectId}</code></aside>}{runDetail.evidence.filter(e => e.schemaVersion === "layrs.run-verdict.v1").map((e, index) => <pre key={index}>{e.log || "Journal vide."}</pre>)}</section>}
    {error && <p className="native-error" role="alert">{error}</p>}
  </details>;
}
