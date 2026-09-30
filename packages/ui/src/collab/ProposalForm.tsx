import { useState, type FormEvent } from "react";
import { Button } from "../components/Button";
import { Banner } from "../components/Feedback";
import { SelectField, TextField } from "../components/Field";
import type { ReviewDraftIdentity } from "../native-review-drafts";
import { useReviewProposalDraft } from "../native-review-proposal-draft-form";
import type { ProposalTarget } from "../native-review-proposal-drafts";
import { namedCaptures } from "./model";
import { CollabError } from "./parts";
import type { CollabTransport, ProjectReferences, ReviewProposal } from "./types";
import { useScopedTasks } from "./useScopedTasks";

const OTHER = "__other";

/**
 * Create a review or propose a new version of an open one. The proposal is a
 * local draft (per server, account, project and exact target version) until an
 * explicit send; a retry reuses the same key, so the server never duplicates it.
 */
export function ProposalForm({
  scope,
  transport,
  spaceId,
  references,
  draftIdentity,
  review,
  writable = true,
  unavailableReason,
  onDone,
  onCancel,
}: {
  scope: string;
  transport: CollabTransport;
  spaceId: string;
  references: ProjectReferences;
  draftIdentity?: ReviewDraftIdentity;
  /** Latest version of the open review for a new version; absent for a new review. */
  review?: ReviewProposal;
  writable?: boolean;
  unavailableReason?: string;
  onDone: (proposal: ReviewProposal) => void;
  onCancel?: () => void;
}) {
  const tasks = useScopedTasks(`${scope}|proposal|${review?.reviewId ?? ""}`);
  const [mode, setMode] = useState(review ? "update" : "new");
  const [error, setError] = useState<unknown>(null);
  const [customStep, setCustomStep] = useState(false);
  const resuming = mode.startsWith("resume:");
  const target: ProposalTarget | null =
    mode === "new" ? { reviewId: null, expectedVersion: 0 } : review ? { reviewId: review.reviewId, expectedVersion: resuming ? Number(mode.slice(7)) : review.version } : null;
  const draft = useReviewProposalDraft(scope, draftIdentity, spaceId, target, review?.reviewId);
  const input = draft.input;
  const captures = namedCaptures(references);
  const knownStep = captures.some((option) => option.stepId === input.stepId);
  const knownLayer = references.layers.some((layer) => layer.layerId === input.layerId);
  const canWrite = mode === "new" || resuming || writable;
  const sending = tasks.isBusy("propose");

  let reason: string | undefined;
  if (!canWrite) reason = unavailableReason ?? "Cette revue n’accepte plus de nouvelle version.";
  else if (!draft.ready) reason = draft.available ? "Restaurez ou supprimez d’abord le brouillon disponible." : "Préparation du brouillon…";
  else if (!input.title.trim()) reason = "Donnez un titre.";
  else if (!input.layerId) reason = "Choisissez le Layer cible.";
  else if (!input.stepId) reason = "Choisissez la capture proposée.";

  function submit(event: FormEvent) {
    event.preventDefault();
    if (reason) return;
    void tasks.run(
      "propose",
      async (alive) => {
        setError(null);
        const sent = draft.submission();
        const proposal = await transport<ReviewProposal>({ action: "propose", review: sent.target.reviewId, request: sent.request });
        draft.clearAfterSuccess(sent);
        if (alive()) onDone(proposal);
      },
      setError,
    );
  }

  return (
    <form className="collab-form" onSubmit={submit} aria-label={review ? "Nouvelle version de la proposition" : "Nouvelle revue"}>
      {review && (
        <SelectField label="Cible de l’envoi" value={mode} disabled={sending} onChange={(event) => setMode(event.target.value)}>
          <option value="update">Nouvelle version après la version {review.version}</option>
          {draft.targets
            .filter((item) => item.expectedVersion < review.version)
            .map((item) => (
              <option key={item.expectedVersion} value={`resume:${item.expectedVersion}`}>
                Reprendre le brouillon qui ciblait la version {item.expectedVersion}
              </option>
            ))}
        </SelectField>
      )}
      {resuming && (
        <p className="collab-muted">
          Reprise avec la version attendue et la clé d’envoi d’origine : le serveur reconnaîtra un envoi déjà accepté ou refusera une proposition devenue
          obsolète, sans la déplacer sur une nouvelle version.
        </p>
      )}
      {draft.available && (
        <Banner
          tone="info"
          title="Brouillon de proposition disponible"
          actions={
            <>
              <Button size="sm" onClick={draft.restore}>
                Restaurer la proposition
              </Button>
              <Button size="sm" variant="ghost" onClick={draft.discard}>
                Supprimer le brouillon
              </Button>
            </>
          }
        >
          Une proposition est conservée sur cet appareil pour ce compte, ce projet et cette cible exacte.
        </Banner>
      )}
      {review && draft.ready && !input.title && !input.layerId && !input.stepId && (
        <div>
          <Button size="sm" variant="ghost" onClick={() => draft.change({ title: review.title, layerId: review.layerId })}>
            Reprendre le titre et le Layer de la version {review.version}
          </Button>
        </div>
      )}
      <TextField label="Titre" value={input.title} maxLength={1024} disabled={!draft.ready || sending} onChange={(event) => draft.change({ title: event.target.value })} />
      <SelectField label="Layer cible" value={knownLayer || !input.layerId ? input.layerId : OTHER} disabled={!draft.ready || sending} onChange={(event) => draft.change({ layerId: event.target.value === OTHER ? input.layerId : event.target.value })}>
        <option value="">Choisir un Layer</option>
        {references.layers.map((layer) => (
          <option key={layer.layerId} value={layer.layerId}>
            {layer.name}
          </option>
        ))}
        {!knownLayer && input.layerId && <option value={OTHER}>Layer du brouillon (non listé)</option>}
      </SelectField>
      <SelectField
        label="Capture proposée"
        hint="Publiez d’abord depuis Desktop la capture à proposer ; seules les captures publiées sont listées."
        value={customStep || (!knownStep && input.stepId) ? OTHER : input.stepId}
        disabled={!draft.ready || sending}
        onChange={(event) => {
          const value = event.target.value;
          setCustomStep(value === OTHER);
          if (value !== OTHER) draft.change({ stepId: value });
        }}
      >
        <option value="">Choisir une capture</option>
        {["Layers", "Versions", "Repères"].map((group) => {
          const items = captures.filter((option) => option.group === group);
          return items.length ? (
            <optgroup key={group} label={group}>
              {items.map((option) => (
                <option key={`${group}:${option.stepId}`} value={option.stepId}>
                  {option.label}
                </option>
              ))}
            </optgroup>
          ) : null;
        })}
        <option value={OTHER}>Autre capture publiée (référence technique)…</option>
      </SelectField>
      {(customStep || (!knownStep && input.stepId)) && (
        <TextField label="Référence de la capture proposée" value={input.stepId} maxLength={256} placeholder="blake3:…" disabled={!draft.ready || sending} onChange={(event) => draft.change({ stepId: event.target.value.trim() })} />
      )}
      <details className="collab-details">
        <summary>Participants (facultatif)</summary>
        <TextField
          label="Références des comptes participants"
          value={input.participants}
          maxLength={32768}
          placeholder="principal:…, principal:…"
          hint="Séparées par des virgules. Le choix par nom arrivera avec l’annuaire des membres du projet."
          disabled={!draft.ready || sending}
          onChange={(event) => draft.change({ participants: event.target.value })}
        />
      </details>
      {draft.notice && (
        <p className="collab-muted" role="status">
          {draft.notice}
        </p>
      )}
      <p className="collab-muted">
        {draft.persistent
          ? "Brouillon récupérable sur cet appareil pendant 30 jours ; l’envoi reste explicite."
          : "La proposition reste en mémoire : une identité vérifiée et le projet exact sont nécessaires pour la conserver sur cet appareil."}
      </p>
      {error ? <CollabError error={error} title="Proposition non envoyée" kept="Votre saisie est conservée ; un nouvel envoi ne crée pas de doublon." /> : null}
      <div className="collab-actions collab-actions--end">
        {draft.key && !draft.available && (
          <Button variant="ghost" onClick={draft.discard} disabled={sending}>
            Supprimer le brouillon
          </Button>
        )}
        {onCancel && (
          <Button onClick={onCancel} disabled={sending}>
            Fermer
          </Button>
        )}
        <Button type="submit" variant="primary" busy={sending} busyLabel="Envoi…" disabledReason={reason}>
          {mode === "new" ? "Créer la revue" : resuming ? "Reprendre l’envoi de la proposition" : "Proposer cette nouvelle version"}
        </Button>
      </div>
    </form>
  );
}
