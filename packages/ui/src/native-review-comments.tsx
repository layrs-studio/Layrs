import { useRef, useState } from "react";
import { commentHistory, type CommentCorrection, type ReviewCommentAction } from "./native-review-comment-history";

export type CommentCorrectionRequest = { idempotencyKey: string; version: number; expectedRevision: number; body: string; reason: string };
type Editor = { action: ReviewCommentAction; expectedRevision: number; body: string; reason: string };
export function NativeReviewComments({ reviewId, version, historical, actions, corrections, principalId, busy, canReply, onReply, onCorrect }: {
  reviewId: string; version: number; historical: boolean; actions: ReviewCommentAction[]; corrections: CommentCorrection[];
  principalId?: string; busy: boolean; canReply: boolean; onReply: (action: string) => void;
  onCorrect: (action: string, request: CommentCorrectionRequest) => Promise<void>;
}) {
  const [editor, setEditor] = useState<Editor | null>(null);
  const [error, setError] = useState("");
  const pending = useRef({ signature: "", key: "" });
  const inFlight = useRef(false);
  async function submit() {
    if (!editor || busy || inFlight.current) return;
    const request = { version: editor.action.version, expectedRevision: editor.expectedRevision, body: editor.body, reason: editor.reason };
    const signature = JSON.stringify([reviewId, editor.action.actionId, request]);
    if (pending.current.signature !== signature) pending.current = { signature, key: crypto.randomUUID() };
    inFlight.current = true; setError("");
    try {
      await onCorrect(editor.action.actionId, { ...request, idempotencyKey: pending.current.key });
      setEditor(null); pending.current = { signature: "", key: "" };
    } catch {
      setError("Correction non confirmée. Vérifiez vos droits et la connexion ; si une autre révision a été publiée, rechargez la revue avant de reprendre. Votre texte reste ici.");
    } finally { inFlight.current = false; }
  }
  return <>
    {actions.filter(action => !historical || action.version === version).map(action => {
      let history: ReturnType<typeof commentHistory> | null = null;
      try { history = commentHistory(reviewId, action, corrections); } catch { /* An invalid chain is never rendered as the current text. */ }
      return <article key={action.actionId}>
        <strong>{action.action} · version {action.version}</strong><code>{action.actorPrincipalId}</code><small>{action.createdAt}</small>
        {action.version !== version && <p>Contexte d’une ancienne version, conservé sans déplacement.</p>}
        {action.anchor && <p>{action.anchor.path}:{action.anchor.startLine}</p>}
        {action.replyTo && <p>Réponse à : {actions.find(item => item.actionId === action.replyTo)?.body.slice(0, 160) || action.replyTo}</p>}
        {!history && <p role="alert">Historique de corrections indisponible. Le texte ci-dessous est l’original.</p>}
        <p style={{ whiteSpace: "pre-wrap" }}>{history?.body ?? action.body}</p>
        {history && history.revision > 0 && <><p>Corrigé · révision {history.revision}</p><details><summary>Historique des corrections ({history.revision})</summary>
          <article><strong>Texte original</strong><code>{action.actorPrincipalId}</code><small>{action.createdAt}</small><p style={{whiteSpace:"pre-wrap"}}>{action.body}</p></article>
          {history.corrections.map(item => <article key={item.evidenceObjectId}><strong>Révision {item.record.revision}</strong><code>{item.record.actorPrincipalId}</code><small>{item.record.correctedAt}</small><p style={{whiteSpace:"pre-wrap"}}>{item.record.body}</p><p>Motif : {item.record.reason}</p><code>{item.evidenceObjectId}</code></article>)}
        </details></>}
        {!historical && action.action === "comment" && action.version === version && <button disabled={busy || !canReply} onClick={() => onReply(action.actionId)}>Répondre</button>}
        {!historical && action.action === "comment" && principalId === action.actorPrincipalId && history && history.revision < 100 && <button disabled={busy} onClick={() => { setEditor({ action, expectedRevision: history.revision, body: history.body, reason: "" }); setError(""); }}>Corriger mon commentaire</button>}
      </article>;
    })}
    {editor && <section aria-label="Correction auditée du commentaire"><h4>Corriger le commentaire · version {editor.action.version}</h4>
      <p>Une nouvelle révision conservera le texte original, son auteur et son ancrage. Elle ne modifie aucune décision de revue.</p>
      <label>Commentaire corrigé<textarea disabled={busy} maxLength={65536} value={editor.body} onChange={event => setEditor({ ...editor, body: event.target.value })} /></label>
      <label>Motif de correction<textarea disabled={busy} maxLength={4096} value={editor.reason} onChange={event => setEditor({ ...editor, reason: event.target.value })} /></label>
      <button disabled={busy || !editor.body.trim() || !editor.reason.trim()} onClick={() => void submit()}>Enregistrer la correction</button>
      <button disabled={busy} onClick={() => { setEditor(null); setError(""); }}>Abandonner la correction</button>
      {error && <p role="alert">{error}</p>}
    </section>}
  </>;
}
