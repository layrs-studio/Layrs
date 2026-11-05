import { useEffect, useRef, useState } from "react";
import { readReviewDraft, removeReviewDraft, saveReviewDraft, type ReviewCommentDraft, type ReviewDraftIdentity, type ReviewDraftInput, type ReviewDraftScope } from "./native-review-drafts";

type Form = ReviewDraftInput & { context: string; initialized: boolean; available: ReviewCommentDraft | null; persistedKey: string | null; notice: string };
type Submission = { context: string; scope: ReviewDraftScope | null; input: ReviewDraftInput; draftKey: string | null };
const empty = (context: string): Form => ({ context, initialized: false, body: "", path: "", line: 1, replyTo: null, available: null, persistedKey: null, notice: "" });

/** No request is made by this hook. Scope changes hide old fields during render,
 * before effects run, including a switch between two accounts on one origin. */
export function useReviewCommentDraft(session: string, identity: ReviewDraftIdentity | undefined, proposal: { spaceId?: string; reviewId: string; version: number } | undefined) {
  const context = JSON.stringify([session, identity?.origin, identity?.principalId, proposal?.spaceId, proposal?.reviewId, proposal?.version]);
  const scope: ReviewDraftScope | null = identity && proposal?.spaceId ? { ...identity, spaceId: proposal.spaceId, reviewId: proposal.reviewId, version: proposal.version } : null;
  const [form, setForm] = useState<Form>(() => empty(context));
  const current = useRef(form);
  const visible = form.context === context ? form : empty(context);
  current.current = visible;
  const currentContext = useRef(context);
  currentContext.current = context;
  function update(next: Form) { current.current = next; setForm(next); }
  useEffect(() => {
    const next = empty(context);
    if (scope) {
      try { next.available = readReviewDraft(localStorage, scope); }
      catch { next.notice = "Le stockage local des brouillons est indisponible ou illisible. Votre saisie reste dans cette fenêtre."; }
    }
    next.initialized = true;
    if (currentContext.current === context) update(next);
    // Values forming scope are all included in the explicit context.
  }, [context]);

  function save(next: Form): Form {
    if (!scope) return { ...next, persistedKey: null };
    try {
      if (!next.body && !next.path && !next.replyTo) {
        if (next.persistedKey) removeReviewDraft(localStorage, scope, next.persistedKey);
        return { ...next, persistedKey: null, notice: "" };
      }
      const draft = saveReviewDraft(localStorage, scope, next);
      return { ...next, persistedKey: draft.idempotencyKey, notice: "Brouillon enregistré sur cet appareil." };
    } catch {
      return { ...next, persistedKey: null, notice: "Brouillon non enregistré : stockage indisponible, plein ou limite de taille atteinte. La saisie reste dans cette fenêtre." };
    }
  }
  function change(patch: Partial<ReviewDraftInput>) {
    if (current.current.context !== context || !current.current.initialized || current.current.available) return;
    update(save({ ...current.current, ...patch }));
  }
  function restore() {
    const selected = current.current.available;
    if (!selected || current.current.context !== context) return;
    // Restoration fills the form only. It does not call save or transport.
    update({ ...empty(context), ...selected, initialized: true, available: null, persistedKey: selected.idempotencyKey, notice: "Brouillon restauré. Vérifiez-le avant de l’envoyer." });
  }
  function discard() {
    if (!scope || current.current.context !== context) return;
    try {
      const removed = removeReviewDraft(localStorage, scope, current.current.available?.idempotencyKey ?? current.current.persistedKey ?? undefined);
      if (!removed) {
        const newer = readReviewDraft(localStorage, scope);
        if (newer) { update({ ...current.current, available: newer, notice: "Un brouillon plus récent est disponible." }); return; }
      }
      update({ ...current.current, available: null, persistedKey: null, notice: "Brouillon local supprimé." });
    } catch { update({ ...current.current, notice: "Le brouillon local n’a pas pu être supprimé." }); }
  }
  function submission(): Submission {
    if (current.current.context !== context || !current.current.initialized || current.current.available) throw new Error("Restaurez ou supprimez d’abord le brouillon disponible.");
    const prepared = save(current.current);
    update(prepared);
    return { context, scope, input: { body: prepared.body, path: prepared.path, line: prepared.line, replyTo: prepared.replyTo }, draftKey: prepared.persistedKey };
  }
  function clearAfterSuccess(sent: Submission) {
    let notice = "";
    let available: ReviewCommentDraft | null = null;
    if (sent.scope && sent.draftKey) {
      try { if (!removeReviewDraft(localStorage, sent.scope, sent.draftKey)) available = readReviewDraft(localStorage, sent.scope); }
      catch { notice = "Commentaire envoyé ; le brouillon local n’a pas pu être supprimé."; }
    }
    if (currentContext.current === sent.context) update({ ...empty(sent.context), initialized: true, available, notice });
  }
  return {
    ...visible, persistent: Boolean(scope), ready: visible.initialized && !visible.available,
    setBody: (body: string) => change({ body }), setPath: (path: string) => change({ path }),
    setLine: (line: number) => change({ line }), setReplyTo: (replyTo: string | null) => change({ replyTo }),
    restore, discard, submission, clearAfterSuccess,
  };
}
