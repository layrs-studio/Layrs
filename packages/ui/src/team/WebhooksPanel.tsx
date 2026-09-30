import { useCallback, useEffect, useRef, useState } from "react";
import { Plus, RefreshCw, Webhook as WebhookIcon } from "lucide-react";
import { Button } from "../components/Button";
import { Badge, Banner, Card, EmptyState, Loading } from "../components/Feedback";
import { Dialog } from "../components/Dialog";
import { TextField } from "../components/Field";
import { useToast } from "../components/Toast";
import { when } from "./time";
import { ConfirmDialog, type Confirmation } from "./ConfirmDialog";
import { describeError } from "./errors";
import { SecretReveal, type RevealedSecret } from "./SecretReveal";
import { eventLabels, webhookEvents, type Delivery, type TeamTransport, type Webhook } from "./types";
import { useTask } from "./useTask";

const deliveryStates: Record<string, { label: string; tone: "success" | "warning" | "danger" | "info" }> = {
  delivered: { label: "Livrée", tone: "success" },
  pending: { label: "En attente", tone: "info" },
  sending: { label: "Envoi en cours", tone: "info" },
  dead: { label: "Tentatives épuisées", tone: "danger" },
};
const deliveryErrors: Record<string, string> = {
  http_rejected: "Le récepteur a répondu par une erreur HTTP.",
  destination_or_transport_failed: "Destination injoignable ou connexion refusée.",
  authority_revoked: "La personne qui a créé le webhook n’administre plus le projet.",
  attempts_exhausted: "Huit tentatives sans succès.",
};

/** P17 · Webhooks: destination, events, one-time secrets, deliveries and replay. */
export function WebhooksPanel({
  scope,
  administration,
  projectName,
  canManage,
}: {
  scope: string;
  administration: TeamTransport;
  projectName: string;
  /** Capability hint `manageWebhooks`; null when unknown. */
  canManage: boolean | null;
}) {
  const task = useTask(scope);
  const toast = useToast();
  const [hooks, setHooks] = useState<Webhook[] | null>(null);
  const [forbidden, setForbidden] = useState(false);
  const [creating, setCreating] = useState(false);
  const [secret, setSecret] = useState<RevealedSecret | null>(null);
  const [confirm, setConfirm] = useState<Confirmation | null>(null);
  const [deliveriesFor, setDeliveriesFor] = useState<Webhook | null>(null);
  const clearSecret = useCallback(() => setSecret(null), []);

  const load = useCallback(
    () =>
      task.run("load", async (live) => {
        try {
          const value = await administration<{ webhooks: Webhook[] }>({ action: "listWebhooks" });
          if (live()) setHooks(value.webhooks);
        } catch (cause) {
          const status = (cause as { status?: unknown })?.status;
          if (status === 403 && live()) {
            setForbidden(true);
            return;
          }
          throw cause;
        }
      }, "Les webhooks n’ont pas pu être lus."),
    [administration, task],
  );

  // Secrets never survive a change of project or account.
  useEffect(() => setSecret(null), [scope]);
  const started = useRef("");
  useEffect(() => {
    if (canManage === false) {
      setForbidden(true);
      return;
    }
    if (started.current === scope) return;
    started.current = scope;
    void load();
  }, [scope, canManage, load]);

  if (forbidden)
    return (
      <EmptyState icon={<WebhookIcon size={22} />} title="Webhooks réservés aux administrateurs du projet">
        Un webhook transmet les événements du projet à un outil externe, signé par un secret partagé.
      </EmptyState>
    );
  return (
    <div className="team-panel team-panel--narrow">
      <div className="team-toolbar">
        <p className="team-muted">
          Chaque événement choisi est envoyé en HTTPS, signé (en-tête <code>x-layrs-signature</code>) avec un secret connu du seul récepteur. Les destinations doivent être
          autorisées dans la configuration de l’instance.
        </p>
        <Button variant="primary" icon={<Plus aria-hidden size={16} />} onClick={() => setCreating(true)}>
          Nouveau webhook
        </Button>
      </div>
      {task.error && <Banner tone="danger">{task.error}</Banner>}
      {!hooks ? (
        <Loading label="Lecture des webhooks…" />
      ) : !hooks.length ? (
        <EmptyState compact icon={<WebhookIcon size={22} />} title="Aucun webhook">
          Ajoutez une destination pour prévenir un outil externe des revues, validations ou changements d’accès.
        </EmptyState>
      ) : (
        <ul className="team-cards" aria-label="Webhooks du projet">
          {hooks.map((hook) => (
            <li key={hook.webhookId}>
              <Card className="team-card">
                <div className="team-card__head">
                  <div className="team-row__main">
                    <p className="team-row__title team-break">{hook.url}</p>
                    <p className="team-row__meta">
                      Secret de génération {hook.secretGeneration}
                      {hook.createdAt && ` · créé le ${when(hook.createdAt)}`}
                    </p>
                  </div>
                  <Badge tone={hook.enabled ? "success" : "info"}>{hook.enabled ? "Actif" : "Désactivé"}</Badge>
                </div>
                <p className="team-row__meta">{hook.eventKinds.map((kind) => eventLabels[kind] ?? kind).join(" · ")}</p>
                <div className="team-actions">
                  <Button size="sm" onClick={() => setDeliveriesFor(hook)}>
                    Livraisons
                  </Button>
                  <Button
                    size="sm"
                    disabledReason={!hook.enabled ? "Webhook désactivé." : undefined}
                    onClick={() =>
                      setConfirm({
                        title: "Renouveler le secret de signature",
                        subject: `${hook.url} · projet ${projectName}`,
                        effects: [
                          `Les prochaines livraisons seront signées avec la génération ${hook.secretGeneration + 1} (en-tête x-layrs-key-generation).`,
                          "Le récepteur doit être mis à jour avec le nouveau secret, sinon il rejettera les livraisons.",
                          "Le nouveau secret s’affichera une seule fois.",
                        ],
                        confirmLabel: "Renouveler le secret",
                        run: async () => {
                          const value = await administration<{ secret: string; secretGeneration: number }>({ action: "rotateWebhook", webhook: hook.webhookId });
                          setSecret({
                            title: "Nouveau secret de signature",
                            owner: `${hook.url} · génération ${value.secretGeneration}`,
                            guidance: "Enregistrez-le dans le récepteur pour vérifier les prochaines livraisons.",
                            value: value.secret,
                          });
                          await load();
                        },
                      })
                    }
                  >
                    Renouveler le secret
                  </Button>
                  {hook.enabled && (
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() =>
                        setConfirm({
                          title: "Désactiver le webhook",
                          subject: `${hook.url} · projet ${projectName}`,
                          effects: [
                            "Plus aucun événement ne sera envoyé à cette adresse.",
                            "La désactivation est définitive : créez un nouveau webhook pour reprendre les envois.",
                            "L’historique des livraisons reste consultable.",
                          ],
                          confirmLabel: "Désactiver",
                          danger: true,
                          run: async () => {
                            await administration({ action: "disableWebhook", webhook: hook.webhookId });
                            toast.show({ tone: "success", title: "Webhook désactivé", description: hook.url });
                            await load();
                          },
                        })
                      }
                    >
                      Désactiver
                    </Button>
                  )}
                </div>
              </Card>
            </li>
          ))}
        </ul>
      )}
      <CreateWebhook
        open={creating}
        onClose={() => setCreating(false)}
        administration={administration}
        projectName={projectName}
        onCreated={(value, url) => {
          setSecret({
            title: "Secret de signature du webhook",
            owner: `${url} · génération ${value.secretGeneration}`,
            guidance: "Copiez-le dans la configuration du récepteur pour vérifier la signature des livraisons.",
            value: value.secret,
          });
          void load();
        }}
      />
      <Deliveries hook={deliveriesFor} onClose={() => setDeliveriesFor(null)} administration={administration} />
      <SecretReveal secret={secret} onDone={clearSecret} />
      <ConfirmDialog value={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}

function CreateWebhook({
  open,
  onClose,
  administration,
  projectName,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  administration: TeamTransport;
  projectName: string;
  onCreated: (value: { secret: string; secretGeneration: number }, url: string) => void;
}) {
  const [url, setUrl] = useState("");
  const [events, setEvents] = useState<string[]>(["review.proposed", "proof.completed"]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    if (!open) return;
    setUrl("");
    setEvents(["review.proposed", "proof.completed"]);
    setError("");
  }, [open]);
  let urlError: string | undefined;
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "https:") urlError = "Adresse HTTPS obligatoire.";
    else if (parsed.search || parsed.hash || parsed.username || parsed.password) urlError = "Sans paramètres, fragment ni identifiants dans l’adresse.";
  } catch {
    urlError = url ? "Adresse invalide." : undefined;
  }
  const invalid = !url ? "Indiquez l’adresse du récepteur." : urlError ?? (!events.length ? "Choisissez au moins un événement." : events.length > 8 ? "Huit événements au plus." : undefined);

  async function submit() {
    setBusy(true);
    setError("");
    try {
      const value = await administration<{ webhookId: string; secret: string; secretGeneration: number }>({ action: "createWebhook", request: { url, eventKinds: events } });
      onCreated(value, url);
      onClose();
    } catch (cause) {
      setError(describeError(cause, "Le webhook n’a pas été créé."));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      open={open}
      onClose={() => !busy && onClose()}
      dismissible={!busy}
      title="Nouveau webhook"
      description={`Projet ${projectName}. Seuls les événements postérieurs à la création sont envoyés.`}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            Annuler
          </Button>
          <Button variant="primary" busy={busy} busyLabel="Création…" disabledReason={invalid} onClick={() => void submit()}>
            Créer le webhook
          </Button>
        </>
      }
    >
      <div className="team-form">
        <TextField
          label="Adresse HTTPS du récepteur"
          type="url"
          value={url}
          maxLength={2048}
          placeholder="https://outils.exemple/webhooks/layrs"
          error={urlError}
          onChange={(event) => setUrl(event.target.value.trim())}
        />
        <fieldset className="team-choices team-choices--compact">
          <legend className="layrs-field__label">Événements transmis ({events.length}/8)</legend>
          {webhookEvents.map((kind) => (
            <label key={kind} className="team-check">
              <input
                type="checkbox"
                checked={events.includes(kind)}
                disabled={!events.includes(kind) && events.length >= 8}
                onChange={(event) => setEvents(event.target.checked ? [...events, kind] : events.filter((item) => item !== kind))}
              />
              {eventLabels[kind]}
            </label>
          ))}
        </fieldset>
        <p className="team-muted team-small">Le secret de signature s’affichera une seule fois après la création.</p>
        {error && <Banner tone="danger">{error}</Banner>}
      </div>
    </Dialog>
  );
}

function Deliveries({ hook, onClose, administration }: { hook: Webhook | null; onClose: () => void; administration: TeamTransport }) {
  const [items, setItems] = useState<Delivery[] | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const generation = useRef(0);

  const load = useCallback(async () => {
    if (!hook) return;
    const current = ++generation.current;
    setBusy("load");
    setError("");
    try {
      const value = await administration<{ deliveries: Delivery[] }>({ action: "webhookDeliveries", webhook: hook.webhookId });
      if (current === generation.current) setItems(value.deliveries);
    } catch (cause) {
      if (current === generation.current) setError(describeError(cause, "Les livraisons n’ont pas pu être lues."));
    } finally {
      if (current === generation.current) setBusy(null);
    }
  }, [administration, hook]);

  useEffect(() => {
    setItems(null);
    if (hook) void load();
  }, [hook, load]);

  return (
    <Dialog open={hook !== null} onClose={onClose} size="lg" title="Dernières livraisons" description={hook?.url}>
      <div className="team-actions">
        <Button size="sm" icon={<RefreshCw aria-hidden size={14} />} busy={busy === "load"} busyLabel="Lecture…" onClick={() => void load()}>
          Actualiser
        </Button>
      </div>
      {error && <Banner tone="danger">{error}</Banner>}
      {!items ? (
        <Loading label="Lecture des livraisons…" />
      ) : !items.length ? (
        <p className="team-muted">Aucune livraison pour le moment.</p>
      ) : (
        <ul className="team-list" aria-label="Livraisons du webhook">
          {items.map((delivery) => {
            const state = deliveryStates[delivery.state] ?? { label: delivery.state, tone: "info" as const };
            return (
              <li key={delivery.deliveryId} className="team-row">
                <div className="team-row__main">
                  <p className="team-row__title">
                    Événement n° {delivery.sequence ?? "—"} · {delivery.attempts} tentative{delivery.attempts > 1 ? "s" : ""}
                    {delivery.lastStatus ? ` · HTTP ${delivery.lastStatus}` : ""}
                  </p>
                  {delivery.lastError && <p className="team-row__meta">{deliveryErrors[delivery.lastError] ?? delivery.lastError}</p>}
                  {delivery.state === "pending" && delivery.nextAttemptAt && <p className="team-row__meta">Prochaine tentative : {when(delivery.nextAttemptAt)}</p>}
                </div>
                <div className="team-row__aside">
                  <Badge tone={state.tone}>{state.label}</Badge>
                  {(delivery.state === "dead" || delivery.state === "delivered") && hook && (
                    <Button
                      size="sm"
                      busy={busy === delivery.deliveryId}
                      busyLabel="Relance…"
                      disabled={busy !== null}
                      onClick={async () => {
                        setBusy(delivery.deliveryId);
                        setError("");
                        try {
                          await administration({ action: "replayWebhook", webhook: hook.webhookId, deliveryId: delivery.deliveryId });
                          setBusy(null);
                          await load();
                        } catch (cause) {
                          setBusy(null);
                          setError(describeError(cause, "La livraison n’a pas été relancée."));
                        }
                      }}
                    >
                      Relancer
                    </Button>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </Dialog>
  );
}
