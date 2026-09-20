import { useEffect, useRef, useState } from "react";
import type { ReviewDraftIdentity } from "./native-review-drafts";
import { readProposalDraft, saveProposalDraft, removeProposalDraft, proposalDraftTargets, proposalRequest, prepareProposalAttempt, type ProposalDraft, type ProposalDraftInput, type ProposalDraftScope, type ProposalTarget } from "./native-review-proposal-drafts";

const blankInput = (): ProposalDraftInput => ({ title: "", layerId: "", stepId: "", participants: "" });
type Form = { context: string; initialized: boolean; input: ProposalDraftInput; available: ProposalDraft | null; key: string | null; notice: string; targets: ProposalTarget[] };
const empty = (context: string): Form => ({ context, initialized: false, input: blankInput(), available: null, key: null, notice: "", targets: [] });
type Submission = { context: string; scope: ProposalDraftScope | null; target: ProposalTarget; key: string; request: ReturnType<typeof proposalRequest> };
export function useReviewProposalDraft(session: string, identity: ReviewDraftIdentity | undefined, spaceId: string | undefined, target: ProposalTarget | null, openReviewId: string | undefined) {
  const context = JSON.stringify([session, identity?.origin, identity?.principalId, spaceId, target?.reviewId, target?.expectedVersion, openReviewId]);
  const scope: ProposalDraftScope | null = identity && spaceId && target ? { ...identity, spaceId, ...target } : null;
  const [form, setForm] = useState<Form>(() => empty(context));
  const visible = form.context === context ? form : empty(context);
  const current = useRef(visible); current.current = visible;
  const active = useRef(context); active.current = context;
  const memoryKey = useRef({ signature: "", key: "" });
  function update(next: Form) { current.current = next; setForm(next); }
  function targets() {
    return identity && spaceId && openReviewId ? proposalDraftTargets(localStorage, { ...identity, spaceId }, openReviewId) : [];
  }
  useEffect(() => {
    const next = empty(context);
    try { if (scope) next.available = readProposalDraft(localStorage, scope); next.targets = targets(); }
    catch { next.notice = "Stockage de proposition indisponible ou illisible. Votre saisie reste dans cette fenêtre."; }
    next.initialized = true;
    if (active.current === context) update(next);
  }, [context]);
  function save(next: Form): Form {
    if (!scope) return { ...next, key: null };
    try {
      if (Object.values(next.input).every(value => !value)) {
        if (next.key) removeProposalDraft(localStorage, scope, next.key);
        return { ...next, key: null, notice: "" };
      }
      const saved = saveProposalDraft(localStorage, scope, next.input);
      return { ...next, key: saved.idempotencyKey, notice: "Brouillon de proposition enregistré sur cet appareil." };
    } catch { return { ...next, key: null, notice: "Proposition non sauvegardée : stockage indisponible, plein ou limite atteinte. La saisie reste dans cette fenêtre." }; }
  }
  function change(patch: Partial<ProposalDraftInput>) {
    if (current.current.context !== context || !current.current.initialized || current.current.available) return;
    update(save({ ...current.current, input: { ...current.current.input, ...patch } }));
  }
  function restore() {
    const selected = current.current.available;
    if (!selected || current.current.context !== context) return;
    update({ ...current.current, available: null, key: selected.idempotencyKey,
      input: { title: selected.title, layerId: selected.layerId, stepId: selected.stepId, participants: selected.participants }, notice: "Proposition restaurée. Vérifiez la cible avant de l’envoyer." });
  }
  function discard() {
    if (!scope || current.current.context !== context) return;
    try {
      const removed = removeProposalDraft(localStorage, scope, current.current.available?.idempotencyKey ?? current.current.key ?? undefined);
      const newer = removed ? null : readProposalDraft(localStorage, scope);
      update({ ...current.current, available: newer, key: null, targets: targets(), notice: newer ? "Une proposition plus récente est disponible." : "Brouillon de proposition supprimé." });
    } catch { update({ ...current.current, notice: "Le brouillon de proposition n’a pas pu être supprimé." }); }
  }
  function submission(): Submission {
    if (!target || current.current.context !== context || !current.current.initialized || current.current.available) throw new Error("Restaurez ou supprimez d’abord le brouillon de proposition.");
    const prepared = save(current.current); update(prepared);
    const attempt = prepareProposalAttempt(context, target, prepared.input, prepared.key, memoryKey.current);
    memoryKey.current = { signature: attempt.signature, key: attempt.key };
    return { context, scope, target, key: attempt.key, request: attempt.request };
  }
  function clearAfterSuccess(sent: Submission) {
    let available: ProposalDraft | null = null;
    let notice = "";
    try { if (sent.scope && !removeProposalDraft(localStorage, sent.scope, sent.key)) available = readProposalDraft(localStorage, sent.scope); }
    catch { notice = "Proposition publiée ; son brouillon local n’a pas pu être supprimé."; }
    if (active.current === sent.context) {
      memoryKey.current = { signature: "", key: "" };
      update({ ...empty(sent.context), initialized: true, available, notice });
    }
  }
  return { ...visible, persistent: Boolean(scope), ready: Boolean(target) && visible.initialized && !visible.available, change, restore, discard, submission, clearAfterSuccess };
}
