import { useRef, useState } from "react";
import { Check, CornerDownRight, FileCode2, MessageSquare, PencilLine, RotateCcw, X, XCircle } from "lucide-react";
import { Button } from "../components/Button";
import { Avatar, Badge, Banner, EmptyState } from "../components/Feedback";
import { TextAreaField, TextField } from "../components/Field";
import { useToast } from "../components/Toast";
import { commentHistory, type CommentCorrection, type ReviewCommentAction } from "../native-review-comment-history";
import type { CommentCorrectionRequest } from "../native-review-comments";
import { useReviewCommentDraft } from "../native-review-draft-form";
import type { ReviewDraftIdentity } from "../native-review-drafts";
import { actionVerb, decisionSuperseded, denied, personAvatarName, personInSentence, personLabel } from "./model";
import { CollabError, TechnicalDetails, When } from "./parts";
import type { CollabCapabilities, CollabTransport, PeopleDirectory, ReviewDetailData, ReviewProposal } from "./types";
import { useScopedTasks } from "./useScopedTasks";

const decisionToasts: Record<string, string> = {
  comment: "Commentaire publié",
  approve: "Approbation enregistrée",
  request_changes: "Demande de modifications enregistrée",
  close: "Revue fermée",
  reopen: "Revue rouverte",
};

/**
 * Discussion of one exact proposal version: thread, audited corrections and
 * the composer. The comment stays in its draft until the server confirms it;
 * a retry reuses the same idempotency key and never duplicates the comment.
 */
export function ReviewDiscussion({
  scope,
  transport,
  spaceId,
  detail,
  shown,
  current,
  people,
  draftIdentity,
  onChanged,
  capabilities,
}: {
  scope: string;
  capabilities?: CollabCapabilities;
  transport: CollabTransport;
  spaceId: string;
  detail: ReviewDetailData;
  shown: ReviewProposal;
  current: ReviewProposal;
  people?: PeopleDirectory;
  draftIdentity?: ReviewDraftIdentity;
  onChanged: () => Promise<void>;
}) {
  const historical = shown.version !== current.version;
  const tasks = useScopedTasks(`${scope}|${shown.reviewId}|${shown.version}`);
  const toast = useToast();
  const [error, setError] = useState<unknown>(null);
  const pending = useRef({ signature: "", key: "" });
  const draft = useReviewCommentDraft(scope, draftIdentity, { spaceId, reviewId: shown.reviewId, version: shown.version });
  const actions = detail.actions.filter((action) => !historical || action.version === shown.version);
  const sending = tasks.isBusy("mutation");
  const integrated = detail.lifecycle === "integrated";
  const closed = detail.lifecycle === "closed";

  function keyed(value: Record<string, unknown>) {
    const signature = JSON.stringify({ review: shown.reviewId, value });
    if (signature !== pending.current.signature) pending.current = { signature, key: crypto.randomUUID() };
    return { ...value, idempotencyKey: pending.current.key };
  }

  function decide(action: string) {
    void tasks.run(
      "mutation",
      async (alive) => {
        setError(null);
        const sent = draft.submission();
        const input = sent.input;
        const payload = {
          version: shown.version,
          action,
          body: input.body,
          anchor: input.path ? { path: input.path, startLine: input.line, endLine: input.line } : null,
          replyTo: action === "comment" ? input.replyTo : null,
        };
        const request = action === "comment" && sent.draftKey ? { ...payload, idempotencyKey: sent.draftKey } : keyed(payload);
        await transport({ action: "decide", review: shown.reviewId, request });
        pending.current = { signature: "", key: "" };
        draft.clearAfterSuccess(sent);
        if (!alive()) return;
        toast.show({ tone: "success", title: decisionToasts[action] ?? "Action enregistrée", description: `Version ${shown.version}` });
        await onChanged();
      },
      setError,
    );
  }

  async function correct(comment: string, request: CommentCorrectionRequest) {
    let failure: unknown = null;
    const ok = await tasks.run(
      "mutation",
      async (alive) => {
        await transport({ action: "correctComment", review: shown.reviewId, comment, request });
        if (alive()) await onChanged();
      },
      (cause) => {
        failure = cause;
      },
    );
    if (!ok) throw failure ?? new Error("Une autre action est en cours.");
  }

  let reason: string | undefined;
  if (historical) reason = "Version historique en lecture seule.";
  else if (denied(capabilities, "proposeReviews")) reason = "Votre compte ne peut pas écrire dans les revues de ce projet.";
  else if (!draft.ready) reason = draft.available ? "Restaurez ou supprimez d’abord le brouillon disponible." : "Préparation du brouillon…";
  const decisionReason =
    reason ??
    (denied(capabilities, "decideReviews") ? "Votre compte ne peut pas approuver ni demander de modifications dans ce projet." : undefined) ??
    (integrated ? "La revue est déjà intégrée." : closed ? "Rouvrez la revue avant de décider." : undefined);
  const replyTarget = draft.replyTo ? detail.actions.find((item) => item.actionId === draft.replyTo) : null;

  return (
    <div className="collab-discussion" role="region" aria-label="Discussion">
      {actions.length ? (
        <ol className="collab-thread" aria-label="Fil de la revue">
          {actions.map((action) => (
            <ThreadItem
              key={action.actionId}
              action={action}
              reviewId={shown.reviewId}
              shownVersion={shown.version}
              currentVersion={current.version}
              historical={historical}
              actions={detail.actions}
              corrections={detail.commentCorrections ?? []}
              people={people}
              busy={sending}
              canReply={draft.ready}
              onReply={() => draft.setReplyTo(action.actionId)}
              onCorrect={correct}
            />
          ))}
        </ol>
      ) : (
        <EmptyState compact icon={<MessageSquare size={20} />} title="Aucun échange pour l’instant">
          Les commentaires et décisions de cette version apparaîtront ici.
        </EmptyState>
      )}

      {!historical && (
        <form
          className="collab-composer"
          aria-label="Répondre dans la revue"
          onSubmit={(event) => {
            event.preventDefault();
            if (!reason && draft.body.trim()) decide("comment");
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.ctrlKey || event.metaKey) && !reason && draft.body.trim()) {
              event.preventDefault();
              decide("comment");
            }
          }}
        >
          {draft.available && (
            <Banner
              tone="info"
              title="Brouillon disponible"
              actions={
                <>
                  <Button size="sm" onClick={draft.restore}>
                    Restaurer le brouillon
                  </Button>
                  <Button size="sm" variant="ghost" onClick={draft.discard}>
                    Supprimer le brouillon local
                  </Button>
                </>
              }
            >
              Un brouillon est conservé sur cet appareil pour ce compte et la version {shown.version}.
            </Banner>
          )}
          {replyTarget && (
            <p className="collab-reply">
              <CornerDownRight aria-hidden size={14} />
              Réponse à {personInSentence(replyTarget.actorPrincipalId, people)} : « {replyTarget.body.slice(0, 120)} »
              <Button size="sm" variant="ghost" icon={<X aria-hidden size={14} />} disabled={sending} onClick={() => draft.setReplyTo(null)}>
                Annuler la réponse
              </Button>
            </p>
          )}
          <TextAreaField
            label="Commentaire"
            value={draft.body}
            maxLength={65536}
            rows={4}
            disabled={!draft.ready || sending}
            hint="Ctrl+Entrée pour commenter. Le texte accompagne aussi une approbation ou une demande de modifications."
            onChange={(event) => draft.setBody(event.target.value)}
          />
          <details className="collab-details">
            <summary>Ancrer à un fichier (facultatif)</summary>
            <div className="collab-inline-fields">
              <TextField label="Chemin exact" value={draft.path} maxLength={4096} disabled={!draft.ready || sending} placeholder="docs/guide.md" onChange={(event) => draft.setPath(event.target.value)} />
              <TextField label="Ligne" type="number" min={1} max={2000000} value={draft.line} disabled={!draft.ready || sending} onChange={(event) => draft.setLine(Number(event.target.value) || 1)} />
            </div>
          </details>
          {error ? <CollabError error={error} title="Envoi non confirmé" kept="Votre texte est conservé. Réessayer ne crée pas de doublon." /> : null}
          {draft.notice && (
            <p className="collab-muted" role="status">
              {draft.notice}
            </p>
          )}
          <div className="collab-composer__actions">
            <div className="collab-actions">
              {closed ? (
                <Button variant="ghost" icon={<RotateCcw aria-hidden size={16} />} disabledReason={reason ?? (integrated ? "La revue est déjà intégrée." : undefined)} onClick={() => decide("reopen")}>
                  Rouvrir la revue
                </Button>
              ) : (
                <Button variant="ghost" icon={<XCircle aria-hidden size={16} />} disabledReason={reason ?? (integrated ? "La revue est déjà intégrée." : undefined)} onClick={() => decide("close")}>
                  Fermer la revue
                </Button>
              )}
              {draft.persistedKey && !draft.available && (
                <Button variant="ghost" disabled={sending} onClick={draft.discard}>
                  Supprimer le brouillon local
                </Button>
              )}
            </div>
            <div className="collab-actions">
              <Button disabledReason={decisionReason} icon={<X aria-hidden size={16} />} onClick={() => decide("request_changes")}>
                Demander des modifications
              </Button>
              <Button disabledReason={decisionReason} icon={<Check aria-hidden size={16} />} onClick={() => decide("approve")}>
                Approuver cette version
              </Button>
              <Button type="submit" variant="primary" busy={sending} busyLabel="Envoi…" disabledReason={reason ?? (!draft.body.trim() ? "Écrivez un commentaire." : undefined)}>
                Commenter
              </Button>
            </div>
          </div>
          <p className="collab-muted">
            {draft.persistent
              ? "Brouillon enregistré sur cet appareil pendant 30 jours, jamais envoyé automatiquement."
              : "Brouillon gardé dans cette fenêtre seulement : vérifiez votre session pour le conserver sur cet appareil."}{" "}
            Approuver ne l’intègre pas : l’intégration est une étape distincte.
          </p>
        </form>
      )}
    </div>
  );
}

type Editor = { expectedRevision: number; body: string; reason: string };

function ThreadItem({
  action,
  reviewId,
  shownVersion,
  currentVersion,
  historical,
  actions,
  corrections,
  people,
  busy,
  canReply,
  onReply,
  onCorrect,
}: {
  action: ReviewCommentAction;
  reviewId: string;
  shownVersion: number;
  currentVersion: number;
  historical: boolean;
  actions: ReviewCommentAction[];
  corrections: CommentCorrection[];
  people?: PeopleDirectory;
  busy: boolean;
  canReply: boolean;
  onReply: () => void;
  onCorrect: (comment: string, request: CommentCorrectionRequest) => Promise<void>;
}) {
  const [editor, setEditor] = useState<Editor | null>(null);
  const [error, setError] = useState(false);
  const pending = useRef({ signature: "", key: "" });
  let history: ReturnType<typeof commentHistory> | null = null;
  try {
    history = commentHistory(reviewId, action, corrections);
  } catch {
    // An invalid correction chain is never rendered as the current text.
  }
  const author = personLabel(action.actorPrincipalId, people);
  const superseded = decisionSuperseded(action, currentVersion);
  const repliedTo = action.replyTo ? actions.find((item) => item.actionId === action.replyTo) : null;
  const own = Boolean(people?.currentPrincipalId) && people?.currentPrincipalId === action.actorPrincipalId;

  async function submit() {
    if (!editor) return;
    const request = { version: action.version, expectedRevision: editor.expectedRevision, body: editor.body, reason: editor.reason };
    const signature = JSON.stringify([reviewId, action.actionId, request]);
    if (pending.current.signature !== signature) pending.current = { signature, key: crypto.randomUUID() };
    setError(false);
    try {
      await onCorrect(action.actionId, { ...request, idempotencyKey: pending.current.key });
      pending.current = { signature: "", key: "" };
      setEditor(null);
    } catch {
      setError(true);
    }
  }

  return (
    <li className={["collab-thread__item", superseded && "collab-thread__item--superseded"].filter(Boolean).join(" ")}>
      <Avatar name={personAvatarName(action.actorPrincipalId, people)} size={32} />
      <article className="collab-thread__body" aria-label={`${author} ${actionVerb(action.action)}`}>
        <header className="collab-thread__head">
          <strong>{author}</strong>
          <span className="collab-muted">{actionVerb(action.action)}</span>
          {action.action === "approve" && !superseded && <Badge tone="success">Approbation</Badge>}
          {action.action === "request_changes" && !superseded && <Badge tone="danger">Modifications demandées</Badge>}
          {superseded && <Badge tone="warning">Décision sur la version {action.version} · ne compte plus</Badge>}
          {!superseded && action.version !== shownVersion && <Badge>Version {action.version}</Badge>}
          <span className="collab-muted">
            <When value={action.createdAt} />
          </span>
        </header>
        {action.anchor && (
          <p className="collab-anchor">
            <FileCode2 aria-hidden size={14} />
            {action.anchor.path} · ligne {action.anchor.startLine}
          </p>
        )}
        {repliedTo && (
          <p className="collab-quote">
            <CornerDownRight aria-hidden size={14} />
            En réponse à {personInSentence(repliedTo.actorPrincipalId, people)} : « {repliedTo.body.slice(0, 160)} »
          </p>
        )}
        {!history && <p role="alert">Historique de corrections indisponible : le texte affiché est l’original.</p>}
        {(history?.body ?? action.body) && <p className="collab-thread__text">{history?.body ?? action.body}</p>}
        {history && history.revision > 0 && (
          <details className="collab-details">
            <summary>Corrigé · historique des corrections ({history.revision})</summary>
            <ol className="collab-corrections">
              <li>
                <strong>Texte original</strong> · <When value={action.createdAt} />
                <p className="collab-thread__text">{action.body}</p>
              </li>
              {history.corrections.map((item) => (
                <li key={item.evidenceObjectId}>
                  <strong>Révision {item.record.revision}</strong> · <When value={item.record.correctedAt} />
                  <p className="collab-thread__text">{item.record.body}</p>
                  <p className="collab-muted">Motif : {item.record.reason}</p>
                </li>
              ))}
            </ol>
          </details>
        )}
        {!historical && action.action === "comment" && (
          <div className="collab-actions">
            {action.version === shownVersion && (
              <Button size="sm" variant="ghost" icon={<CornerDownRight aria-hidden size={14} />} disabledReason={busy || !canReply ? "Envoi ou brouillon en attente." : undefined} onClick={onReply}>
                Répondre
              </Button>
            )}
            {own && history && history.revision < 100 && !editor && (
              <Button
                size="sm"
                variant="ghost"
                icon={<PencilLine aria-hidden size={14} />}
                disabledReason={busy ? "Une autre action est en cours." : undefined}
                onClick={() => {
                  setEditor({ expectedRevision: history.revision, body: history.body, reason: "" });
                  setError(false);
                }}
              >
                Corriger mon commentaire
              </Button>
            )}
          </div>
        )}
        {editor && (
          <section className="collab-correction" aria-label="Correction auditée du commentaire">
            <p className="collab-muted">La correction conserve le texte original, son auteur et son ancrage. Elle ne modifie aucune décision.</p>
            <TextAreaField label="Commentaire corrigé" value={editor.body} maxLength={65536} disabled={busy} onChange={(event) => setEditor({ ...editor, body: event.target.value })} />
            <TextField label="Motif de correction" value={editor.reason} maxLength={4096} disabled={busy} onChange={(event) => setEditor({ ...editor, reason: event.target.value })} />
            {error && (
              <Banner tone="danger" title="Correction non confirmée">
                Vérifiez vos droits et la connexion ; si une autre révision a été publiée, rechargez la revue avant de reprendre. Votre texte reste ici.
              </Banner>
            )}
            <div className="collab-actions">
              <Button size="sm" disabled={busy} onClick={() => setEditor(null)}>
                Abandonner la correction
              </Button>
              <Button size="sm" variant="primary" busy={busy} busyLabel="Enregistrement…" disabledReason={!editor.body.trim() || !editor.reason.trim() ? "Indiquez le texte corrigé et un motif." : undefined} onClick={() => void submit()}>
                Enregistrer la correction
              </Button>
            </div>
          </section>
        )}
        <TechnicalDetails items={[{ label: "Auteur", value: action.actorPrincipalId, copy: "Copier" }, { label: "Action", value: action.actionId, copy: "Copier" }]} />
      </article>
    </li>
  );
}
