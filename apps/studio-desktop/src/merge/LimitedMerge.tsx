import { useEffect, useRef, useState } from "react";
import { CheckCircle2, FileWarning, GitMerge, History } from "lucide-react";
import { Badge, Banner, Button, Card, EmptyState, PageHeader, SelectField, TechnicalId, TextAreaField, TextField, useToast } from "@layrs/ui";
import { useProjection } from "../app/projection";
import { useRemote } from "../app/remote";
import { useWorkspace } from "../app/workspace";
import { nativeProjection } from "../tauri";
import { bindPartialMergeChoices, clearPartialMergeChoices, loadPartialMergeChoices, savePartialMergeChoices, type PartialMergeChoice } from "./partialMergeChoices";

type RebasePreview = {
  historical?: { stepId: string; paths: string[] } | null;
  files: { path: string; conflict: boolean; automatic: unknown }[];
  local: { captureSequence: number; target: { baseStepId: string } };
};

const choiceLabels: Record<PartialMergeChoice, string> = {
  local: "Ma version locale",
  remote: "Version distante",
  base: "Version de départ",
  delete: "Supprimer le fichier",
};

/**
 * Limited access: rebase of the local capture on the published Layer, limited
 * to the cases the engine supports. Choices are persisted on this computer,
 * bound to the exact preview; another preview never reuses them.
 */
export function LimitedMerge() {
  const workspace = useWorkspace();
  const projection = useProjection();
  const remote = useRemote();
  const toast = useToast();
  const { data, patch, ready, target } = projection;
  const [preview, setPreview] = useState<RebasePreview | null>(null);
  const [choices, setChoices] = useState<Record<string, PartialMergeChoice>>({});
  const [message, setMessage] = useState("");
  const [historicalStep, setHistoricalStep] = useState("");
  const [historicalPaths, setHistoricalPaths] = useState("");
  const binding = useRef<Awaited<ReturnType<typeof bindPartialMergeChoices>> | null>(null);
  const origin = data?.identity.origin ?? "";
  const root = workspace.root;

  useEffect(() => {
    binding.current = null;
    setPreview(null);
    setChoices({});
    setMessage("");
  }, [root, origin]);

  const sequence = patch?.sequence ?? null;
  const blocked = workspace.busy
    ? `Opération en cours : ${workspace.busy}`
    : !data || !ready
      ? "Actualisez l’accès limité avant de continuer."
      : data.pendingPublication
        ? "Une publication attend son reçu : relancez-la d’abord depuis Synchronisation."
        : undefined;
  const prepareReason =
    blocked ??
    (sequence === null
      ? "Capturez d’abord vos modifications autorisées."
      : !target?.layerId
        ? "Choisissez le Layer distant cible dans Synchronisation."
        : remote.session.state !== "connected"
          ? "Connectez-vous d’abord au serveur."
          : undefined);

  async function accept(value: RebasePreview | null, revision: number, notice: string) {
    const next = value ? await bindPartialMergeChoices(root, origin, value) : null;
    if (!projection.isCurrent(revision)) return;
    binding.current = next;
    setPreview(value);
    setChoices({});
    setMessage(notice);
    if (value && next)
      try {
        setChoices(loadPartialMergeChoices(localStorage, next, value));
      } catch {
        setMessage(`${notice} Les choix précédents ne peuvent pas être rechargés ; sélectionnez-les à nouveau.`);
      }
  }

  function choose(path: string, choice: PartialMergeChoice) {
    if (!preview || !binding.current || workspace.busy) return;
    const next = { ...choices, [path]: choice };
    setChoices(next);
    try {
      savePartialMergeChoices(localStorage, binding.current, preview, next);
      setMessage("Choix enregistré sur ce poste pour ce brouillon.");
    } catch {
      setMessage("Choix conservé en mémoire, mais impossible de l’enregistrer sur ce poste. Gardez cette fenêtre ouverte jusqu’à l’application.");
    }
  }

  const prepare = () =>
    projection.perform("Préparation de la fusion…", async (revision) => {
      if (sequence === null || !target?.layerId) return;
      const value = await remote.transport<RebasePreview>({ action: "previewProjectionRebase", patchId: sequence, layerId: target.layerId });
      await accept(value, revision, "Brouillon enregistré. Vos fichiers restent inchangés jusqu’à l’application.");
    });

  const reload = () =>
    projection.perform("Lecture du brouillon…", async (revision) => {
      const value = await nativeProjection<RebasePreview | null>(root, { action: "rebaseDraft" });
      await accept(value, revision, value ? "Brouillon et choix locaux rechargés." : "Aucun brouillon de fusion enregistré.");
    });

  const prepareHistorical = () =>
    projection.perform("Préparation de la restauration…", async (revision) => {
      if (sequence === null || !target?.layerId || !historicalStep.trim()) return;
      const paths = [...new Set(historicalPaths.split(/\r?\n/).map((path) => path.trim()).filter(Boolean))].sort();
      const value = await remote.transport<RebasePreview>({
        action: "previewProjectionHistoricalRestore",
        patchId: sequence,
        layerId: target.layerId,
        selection: { stepId: historicalStep.trim(), paths },
      });
      await accept(value, revision, "Aperçu historique enregistré. Vérifiez les fichiers avant de restaurer.");
    });

  async function apply() {
    let applied = false;
    await projection.perform("Application de la fusion…", async (revision) => {
      if (!preview) return;
      const historical = Boolean(preview.historical);
      await remote.transport({ action: "applyProjectionRebase", preview, choices });
      if (!projection.isCurrent(revision)) return;
      if (binding.current)
        try {
          clearPartialMergeChoices(localStorage, binding.current);
        } catch {
          // The applied operation is authoritative; a stale cache cannot match a new preview.
        }
      binding.current = null;
      setPreview(null);
      setChoices({});
      const text = historical
        ? "Version historique restaurée et capturée localement. Publiez la nouvelle capture pour la partager."
        : "Fusion appliquée et capturée localement. Publiez la nouvelle capture pour la partager.";
      setMessage(text);
      toast.show({ tone: "success", title: historical ? "Version restaurée" : "Fusion appliquée", description: "Publiez la nouvelle capture pour la partager." });
      applied = true;
    });
    // Read the new local state; an error stays displayed instead.
    if (applied) await projection.refresh();
  }

  const conflicts = preview?.files.filter((file) => file.conflict) ?? [];
  const unresolved = conflicts.filter((file) => !Object.hasOwn(choices, file.path)).length;

  return (
    <div className="layrs-page">
      <PageHeader
        title="Fusion"
        description="Rejouer votre capture sur la dernière version publiée des fichiers autorisés. Les modifications de texte compatibles sont réunies automatiquement."
        aside={preview && conflicts.length > 0 && <p className="merge__progress" role="status">{`${conflicts.length - unresolved} sur ${conflicts.length} résolu${conflicts.length - unresolved > 1 ? "s" : ""}`}</p>}
      />
      <Banner tone="info" title="Accès limité : fusion restreinte aux cas pris en charge">
        Seuls les fichiers autorisés sont comparés ; les chemins réservés restent intacts. Aucune fusion structurelle générale n’est promise.
      </Banner>
      <section aria-label="Fusion de l’accès limité" className="limited-merge">
        <Card className="merge__prepare">
          <div className="merge__prepare-head">
            <GitMerge aria-hidden size={22} />
            <div>
              <h2>Préparer la fusion</h2>
              <p className="sync-muted">
                Capture locale {sequence ?? "—"} vers <strong>{target?.name || target?.layerId || "Layer distant non choisi"}</strong>. Rien n’est modifié avant
                l’application.
              </p>
            </div>
          </div>
          <div className="merge__prepare-actions">
            <Button variant="primary" disabledReason={prepareReason} onClick={() => void prepare()}>
              Préparer la fusion
            </Button>
            <Button disabledReason={blocked} onClick={() => void reload()}>
              Recharger le brouillon de fusion
            </Button>
          </div>
          <details className="merge__advanced">
            <summary>
              <History aria-hidden size={14} /> Restaurer une version publiée
            </summary>
            <p className="sync-muted">
              Les fichiers ordinaires actuellement modifiables peuvent être restaurés. Une liste vide sélectionne tous ces fichiers ; les fichiers protégés
              restent réservés. La version distante reste inchangée jusqu’à publication.
            </p>
            <TextField label="Identifiant de la version publiée" value={historicalStep} placeholder="blake3:…" onChange={(event) => setHistoricalStep(event.target.value)} />
            <TextAreaField label="Chemins historiques à restaurer, un par ligne" value={historicalPaths} rows={3} onChange={(event) => setHistoricalPaths(event.target.value)} />
            <div>
              <Button disabledReason={prepareReason ?? (!historicalStep.trim() ? "Indiquez la version publiée." : undefined)} onClick={() => void prepareHistorical()}>
                Préparer la restauration historique
              </Button>
            </div>
          </details>
        </Card>

        {message && <Banner tone="info" title={message} />}

        {preview ? (
          <Card className="limited-merge__preview">
            <header className="limited-merge__head">
              <h2>{preview.historical ? "Restauration historique proposée" : "Résultat proposé"}</h2>
              <span className="sync-muted">
                Capture {preview.local.captureSequence} · nouvelle base <TechnicalId value={preview.local.target.baseStepId} label="Copier" />
              </span>
            </header>
            <ul className="limited-merge__files">
              {preview.files.map((file) => (
                <li key={file.path} className="limited-merge__file">
                  {file.conflict ? <FileWarning aria-hidden size={18} className="limited-merge__warn" /> : <CheckCircle2 aria-hidden size={18} className="conflict__resolved" />}
                  <code className="limited-merge__path">{file.path}</code>
                  {file.conflict ? (
                    <SelectField
                      label={`Version à conserver pour ${file.path}`}
                      hideLabel
                      value={Object.hasOwn(choices, file.path) ? choices[file.path] : ""}
                      disabled={Boolean(workspace.busy)}
                      onChange={(event) => choose(file.path, event.target.value as PartialMergeChoice)}
                    >
                      <option value="" disabled>
                        Choisir une résolution
                      </option>
                      {(Object.keys(choiceLabels) as PartialMergeChoice[]).map((choice) => (
                        <option key={choice} value={choice}>
                          {choiceLabels[choice]}
                        </option>
                      ))}
                    </SelectField>
                  ) : (
                    <Badge tone="success">{file.automatic === null ? "Suppression résolue" : "Résolu automatiquement"}</Badge>
                  )}
                </li>
              ))}
            </ul>
            <div className="limited-merge__actions">
              <p className="sync-muted">
                {unresolved ? `${unresolved} conflit${unresolved > 1 ? "s" : ""} à choisir avant l’application.` : "Tous les conflits ont un choix."} Les choix
                restent attachés à cet aperçu.
              </p>
              <Button
                variant="primary"
                busy={workspace.busy === "Application de la fusion…"}
                busyLabel="Application…"
                disabledReason={blocked ?? (unresolved ? "Choisissez une version pour chaque conflit." : undefined)}
                onClick={() => void apply()}
              >
                {preview.historical ? "Appliquer la restauration historique" : "Appliquer la fusion résolue"}
              </Button>
            </div>
          </Card>
        ) : (
          <EmptyState compact icon={<GitMerge size={22} />} title="Aucun brouillon ouvert">
            Préparez la fusion après avoir capturé vos modifications, ou rechargez un brouillon enregistré sur ce poste.
          </EmptyState>
        )}
      </section>
    </div>
  );
}
