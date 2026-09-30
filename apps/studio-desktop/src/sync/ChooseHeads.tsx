import { useRef, useState } from "react";
import { GitMerge } from "lucide-react";
import { Banner, Button, SelectField, TechnicalId, TextAreaField } from "@layrs/ui";
import { explainError } from "../app/errors";
import { useRemote } from "../app/remote";
import { useWorkspace } from "../app/workspace";

type ChoosePreview = {
  origin: string;
  spaceId: string;
  layerId: string;
  expectedViewId: string;
  expectedGeneration: number;
  previousHeads: string[];
  chosenHeads: string[];
  discardedHeads: string[];
  decisionProductObjectId: string;
};

function HeadList({ title, heads }: { title: string; heads: string[] }) {
  return (
    <div className="choose__list">
      <h4>{title}</h4>
      {heads.length ? (
        <ul>
          {heads.map((head) => (
            <li key={head}>
              <TechnicalId value={head} label="Copier" />
            </li>
          ))}
        </ul>
      ) : (
        <p className="sync-muted">Aucune</p>
      )}
    </div>
  );
}

/**
 * Explicit replacement of the published heads of a Layer (after a squash or a
 * replay). Previewed against a server generation; a changed server refuses it.
 */
export function ChooseHeads({ locked }: { locked?: string }) {
  const workspace = useWorkspace();
  const remote = useRemote();
  const layers = workspace.data?.state.layers ?? [];
  const [layer, setLayer] = useState(workspace.activeLayer?.layerId ?? layers[0]?.layerId ?? "");
  const [heads, setHeads] = useState("");
  const [reason, setReason] = useState("");
  const [preview, setPreview] = useState<ChoosePreview | null>(null);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [pending, setPending] = useState(false);
  const inFlight = useRef(false);

  function invalidate() {
    setPreview(null);
    setMessage("");
    setError("");
  }

  async function perform(label: string, work: () => Promise<void>) {
    if (inFlight.current || !workspace.acquire(label)) return;
    inFlight.current = true;
    setPending(true);
    setError("");
    setMessage("");
    try {
      await work();
    } catch (cause) {
      setError(explainError(cause, "remote").detail);
    } finally {
      inFlight.current = false;
      setPending(false);
      workspace.release();
    }
  }

  const blocked = locked ?? (pending ? "Opération en cours." : undefined);
  return (
    <details>
      <summary>
        <GitMerge aria-hidden size={16} /> Captures candidates et têtes publiées
      </summary>
      <p>
        Après un regroupement ou une reprise de changements, envoyez d’abord les captures candidates : elles sont admises sans changer les têtes du serveur.
        Un administrateur peut ensuite remplacer explicitement les têtes publiées d’un Layer. Les anciennes captures restent dans l’historique et vos
        fichiers locaux ne changent pas ; utilisez ensuite Recevoir.
      </p>
      <Button
        disabledReason={blocked}
        onClick={() => {
          invalidate();
          void remote.operate(
            { kind: "candidates", label: "Envoi des captures candidates", busy: "Envoi des captures candidates…", context: "candidates", cancellable: true },
            { action: "publishCandidates" },
          );
        }}
      >
        Envoyer les captures candidates
      </Button>
      <div className="choose">
        <SelectField
          label="Layer distant à modifier"
          value={layer}
          onChange={(event) => {
            setLayer(event.target.value);
            invalidate();
          }}
        >
          {layers.map((item) => (
            <option key={item.layerId} value={item.layerId}>
              {item.name}
            </option>
          ))}
        </SelectField>
        <TextAreaField
          label="Captures à conserver comme têtes"
          value={heads}
          rows={3}
          hint="Un identifiant blake3:… par ligne. Vide pour retirer toutes les têtes."
          onChange={(event) => {
            setHeads(event.target.value);
            invalidate();
          }}
        />
        <TextAreaField
          label="Motif du choix distant"
          value={reason}
          rows={2}
          maxLength={4096}
          onChange={(event) => {
            setReason(event.target.value);
            invalidate();
          }}
        />
        <div>
          <Button
            disabledReason={blocked ?? (!layer ? "Choisissez un Layer." : !reason.trim() ? "Indiquez le motif du choix." : undefined)}
            onClick={() =>
              void perform("Préparation de l’aperçu…", async () => {
                const chosenHeads = [...new Set(heads.split(/\s+/).filter(Boolean))].sort();
                const value = await remote.transport<{ preview: ChoosePreview }>({ action: "previewChoose", layer, chosenHeads, reason });
                setPreview(value.preview);
              })
            }
          >
            Prévisualiser le choix distant
          </Button>
        </div>
      </div>
      {preview && (
        <section aria-label="Aperçu du choix distant" className="choose__preview">
          <h3>Effet sur les têtes du serveur</h3>
          <p className="sync-muted">Vue attendue : génération {preview.expectedGeneration}.</p>
          <div className="choose__lists">
            <HeadList title="Têtes actuelles" heads={preview.previousHeads} />
            <HeadList title="Têtes après application" heads={preview.chosenHeads} />
            <HeadList title="Têtes écartées explicitement" heads={preview.discardedHeads} />
          </div>
          {!preview.chosenHeads.length && <p>Le Layer n’aura plus aucune tête publiée. Ses anciennes captures restent conservées.</p>}
          <p className="sync-muted">Si le serveur a changé, cette décision sera refusée. Après une réponse perdue, refaites cet aperçu pour retrouver le même reçu.</p>
          <Button
            variant="primary"
            disabledReason={blocked}
            onClick={() =>
              void perform("Application du choix distant…", async () => {
                const result = await remote.transport<{ receipt: { status: string; rejections?: { code: string }[] } }>({ action: "applyChoose", preview });
                if (result.receipt.status !== "applied") {
                  if (result.receipt.rejections?.some((item) => item.code === "gate_rejected"))
                    throw new Error("Décision refusée par les protections du Layer. Votre aperçu est conservé.");
                  throw new Error("Décision refusée par le serveur. Vérifiez vos droits, puis recréez l’aperçu si la vue a changé.");
                }
                setPreview(null);
                setMessage("Choix publié. Utilisez Recevoir pour examiner la nouvelle vue distante.");
              })
            }
          >
            Appliquer ce choix sur le serveur
          </Button>
        </section>
      )}
      {message && (
        <Banner tone="success" title={message} />
      )}
      {error && <Banner tone="danger" title={error} />}
    </details>
  );
}
