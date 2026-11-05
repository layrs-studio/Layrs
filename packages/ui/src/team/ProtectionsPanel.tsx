import { useEffect, useMemo, useState } from "react";
import { ShieldCheck } from "lucide-react";
import { Button } from "../components/Button";
import { Banner, Card, EmptyState, Loading } from "../components/Feedback";
import { Checkbox, SelectField, TextAreaField, TextField } from "../components/Field";
import { useToast } from "../components/Toast";
import { when } from "./time";
import { ConfirmDialog, type Confirmation } from "./ConfirmDialog";
import { personName, type LayerRef, type Person, type Protection, type TeamTransport } from "./types";
import type { Directory } from "./useDirectory";
import { useTask } from "./useTask";

type Form = {
  approvals: number;
  selfApproval: boolean;
  rejectConflicts: boolean;
  requireCI: boolean;
  recipe: string;
  runners: string[];
  proofHours: number;
};

const EMPTY: Form = { approvals: 1, selfApproval: false, rejectConflicts: true, requireCI: false, recipe: "", runners: [], proofHours: 24 };
const PRINCIPAL = /^principal:[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$/;

function fromProtection(value: Protection | null): Form {
  if (!value) return EMPTY;
  return {
    approvals: value.minimumApprovals,
    selfApproval: value.allowSelfApproval,
    rejectConflicts: value.rejectConflicts,
    requireCI: Boolean(value.recipeObjectId),
    recipe: "",
    runners: [...value.trustedRunnerPrincipalIds],
    proofHours: Math.round((value.maximumProofAgeSeconds / 3600) * 100) / 100,
  };
}

/**
 * P17 · Protections: integration policy of one Layer (approvals, conflicts)
 * and its automatic validation requirement. The server stores the recipe by
 * reference only, so keeping CI while changing anything requires the exact
 * recipe again; the form says so rather than silently dropping it.
 */
export function ProtectionsPanel({
  scope,
  directory,
  administration,
  layers,
  projectName,
  canConfigure,
  initialLayer,
  onDirtyChange,
}: {
  scope: string;
  directory: Directory;
  administration: TeamTransport;
  layers: LayerRef[];
  projectName: string;
  /** Capability hint `configureProtection`; null when unknown. */
  canConfigure: boolean | null;
  initialLayer?: string;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const task = useTask(scope);
  const toast = useToast();
  const [layerId, setLayerId] = useState(initialLayer && layers.some((layer) => layer.layerId === initialLayer) ? initialLayer : (layers[0]?.layerId ?? ""));
  const [current, setCurrent] = useState<Protection | null>(null);
  const [loadedFor, setLoadedFor] = useState("");
  const [form, setForm] = useState<Form>(EMPTY);
  const [manualRunner, setManualRunner] = useState("");
  const [confirm, setConfirm] = useState<Confirmation | null>(null);
  const layer = layers.find((item) => item.layerId === layerId);
  const baseline = useMemo(() => fromProtection(current), [current]);
  const dirty = loadedFor === layerId && JSON.stringify(form) !== JSON.stringify(baseline);

  useEffect(() => onDirtyChange?.(dirty), [dirty, onDirtyChange]);
  useEffect(() => () => onDirtyChange?.(false), [onDirtyChange]);

  useEffect(() => {
    if (!layerId) return;
    void task.run("load", async (live) => {
      const value = await administration<Protection | null>({ action: "protection", layer: layerId });
      if (!live()) return;
      setCurrent(value);
      setForm(fromProtection(value));
      setLoadedFor(layerId);
    }, "La protection n’a pas pu être lue.");
  }, [layerId, scope]);

  const services: Person[] = useMemo(
    () => directory.accounts.filter((account) => account.kind === "service" && account.status === "active"),
    [directory.accounts],
  );
  let recipeError: string | undefined;
  let recipe: unknown = null;
  if (form.requireCI && form.recipe.trim()) {
    try {
      recipe = JSON.parse(form.recipe);
      if (!recipe || typeof recipe !== "object" || Array.isArray(recipe)) recipeError = "La recette doit être un objet JSON.";
      else if (new TextEncoder().encode(form.recipe).length > 64 * 1024) recipeError = "La recette dépasse 64 Kio.";
    } catch {
      recipeError = "JSON invalide.";
    }
  }
  const invalid = !layerId
    ? "Choisissez un Layer."
    : form.approvals < 0 || form.approvals > 10 || !Number.isInteger(form.approvals)
      ? "Entre 0 et 10 approbations."
      : form.requireCI && !form.recipe.trim()
        ? current?.recipeObjectId
          ? "Collez la recette exacte actuelle pour conserver la validation automatique, ou décochez-la."
          : "Collez la recette de validation."
        : recipeError
          ? recipeError
          : form.requireCI && !form.runners.length
            ? "Choisissez au moins un exécuteur de confiance."
            : form.requireCI && (form.proofHours < 1 / 60 || form.proofHours > 720)
              ? "Âge maximal entre 1 minute et 720 heures."
              : !dirty
                ? "Aucune modification à enregistrer."
                : undefined;
  const blocked = canConfigure === false ? "Réservé aux administrateurs du projet." : undefined;

  function save() {
    const request = {
      minimumApprovals: form.approvals,
      allowSelfApproval: form.selfApproval,
      rejectConflicts: form.rejectConflicts,
      recipe: form.requireCI ? recipe : null,
      trustedRunnerPrincipalIds: form.requireCI ? form.runners : [],
      maximumProofAgeSeconds: Math.max(60, Math.round((form.requireCI ? form.proofHours : baseline.proofHours || 24) * 3600)),
    };
    setConfirm({
      title: "Enregistrer la protection",
      subject: `Layer ${layer?.name ?? layerId} · projet ${projectName}`,
      effects: [
        `${form.approvals} approbation${form.approvals > 1 ? "s" : ""} humaine${form.approvals > 1 ? "s" : ""} requise${form.approvals > 1 ? "s" : ""}${form.selfApproval ? ", l’auteur peut approuver" : ", hors auteur"}.`,
        form.rejectConflicts ? "Les conflits non résolus sont refusés." : "Les conflits non résolus ne bloquent pas l’intégration.",
        form.requireCI
          ? `Validation automatique exigée sur la capture exacte, par ${form.runners.map((id) => personName(directory.people, id)).join(", ")}, preuve de moins de ${form.proofHours} h.`
          : current?.recipeObjectId
            ? "La validation automatique actuelle est retirée."
            : "Aucune validation automatique exigée.",
        "Le serveur l’applique aux écritures directes et aux intégrations de ce Layer ; les preuves obtenues sous l’ancienne règle ne comptent plus.",
      ],
      confirmLabel: "Enregistrer",
      run: async () => {
        const saved = await administration<Protection>({ action: "configureProtection", layer: layerId, request });
        setCurrent(saved);
        setForm(fromProtection(saved));
        toast.show({ tone: "success", title: "Protection enregistrée", description: `Layer ${layer?.name ?? layerId}.` });
      },
    });
  }

  if (!layers.length)
    return (
      <EmptyState icon={<ShieldCheck size={22} />} title="Aucun Layer publié">
        Les protections se règlent par Layer, une fois le projet publié.
      </EmptyState>
    );
  return (
    <div className="team-panel team-panel--narrow">
      {blocked && <Banner tone="info">Vous consultez les protections en lecture seule : leur modification est réservée aux administrateurs du projet.</Banner>}
      <SelectField
        label="Layer"
        value={layerId}
        onChange={(event) => {
          if (dirty && !window.confirm("Abandonner les modifications non enregistrées de ce Layer ?")) return;
          setLayerId(event.target.value);
        }}
        hint={dirty ? "Modifications non enregistrées sur ce Layer." : undefined}
      >
        {layers.map((item) => (
          <option key={item.layerId} value={item.layerId}>
            {item.name}
          </option>
        ))}
      </SelectField>
      {task.error && <Banner tone="danger">{task.error}</Banner>}
      {task.pending === "load" && loadedFor !== layerId ? (
        <Loading label="Lecture de la protection…" />
      ) : (
        <>
          <Card className="settings-card">
            <h3 className="team-section-title">Revue et intégration</h3>
            {current ? (
              <p className="team-muted team-small">
                Configurée le {current.configuredAt ? when(current.configuredAt) : "—"}
                {current.configuredBy && ` par ${personName(directory.people, current.configuredBy)}`}.
              </p>
            ) : (
              <p className="team-muted team-small">Aucune protection enregistrée : ce Layer accepte les écritures selon les rôles.</p>
            )}
            <TextField
              label="Approbations humaines requises"
              type="number"
              min={0}
              max={10}
              value={form.approvals}
              onChange={(event) => setForm({ ...form, approvals: Number(event.target.value) })}
              disabled={Boolean(blocked)}
            />
            <Checkbox label="Autoriser l’auteur à approuver sa propre proposition" checked={form.selfApproval} onChange={(event) => setForm({ ...form, selfApproval: event.target.checked })} disabled={Boolean(blocked)} />
            <Checkbox label="Refuser les conflits non résolus" checked={form.rejectConflicts} onChange={(event) => setForm({ ...form, rejectConflicts: event.target.checked })} disabled={Boolean(blocked)} />
          </Card>
          <Card className="settings-card">
            <h3 className="team-section-title">Validation automatique</h3>
            <Checkbox
              label="Exiger une preuve de validation pour la capture exacte"
              hint="Un compte de service de confiance exécute la recette et dépose une preuve avant l’intégration."
              checked={form.requireCI}
              onChange={(event) => setForm({ ...form, requireCI: event.target.checked })}
              disabled={Boolean(blocked)}
            />
            {form.requireCI && (
              <>
                {current?.recipeObjectId && (
                  <Banner tone="info" title="Recette actuelle conservée par référence">
                    Le serveur ne renvoie pas le contenu de la recette. Pour enregistrer une modification en gardant la validation, collez la recette exacte ; sinon décochez
                    l’exigence. <code className="team-code">{current.recipeObjectId}</code>
                  </Banner>
                )}
                <TextAreaField
                  label="Recette JSON"
                  rows={6}
                  maxLength={65536}
                  spellCheck={false}
                  value={form.recipe}
                  error={recipeError}
                  placeholder='{"image":"…@sha256:…","command":["/bin/sh","-c","./ci/test.sh"],"timeoutSeconds":300,"maximumLogBytes":65536}'
                  onChange={(event) => setForm({ ...form, recipe: event.target.value })}
                  disabled={Boolean(blocked)}
                />
                <fieldset className="team-picker" disabled={Boolean(blocked)}>
                  <legend className="layrs-field__label">Exécuteurs de confiance</legend>
                  <p className="layrs-field__hint">Comptes de service actifs, membres du projet avec le rôle Automatisation.</p>
                  <ul className="team-picker__list team-picker__list--checks">
                    {[...new Set([...services.map((item) => item.principalId), ...form.runners])].map((id) => (
                      <li key={id}>
                        <label className="team-check">
                          <input
                            type="checkbox"
                            checked={form.runners.includes(id)}
                            onChange={(event) => setForm({ ...form, runners: event.target.checked ? [...form.runners, id] : form.runners.filter((item) => item !== id) })}
                          />
                          {personName(directory.people, id)}
                        </label>
                      </li>
                    ))}
                  </ul>
                  <details className="team-picker__manual">
                    <summary>Ajouter un exécuteur par son identifiant</summary>
                    <div className="team-inline">
                      <TextField
                        label="Identifiant technique du compte de service"
                        placeholder="principal:…"
                        value={manualRunner}
                        error={manualRunner && !PRINCIPAL.test(manualRunner.trim()) ? "Format attendu : principal:…" : undefined}
                        onChange={(event) => setManualRunner(event.target.value)}
                      />
                      <Button
                        disabledReason={!PRINCIPAL.test(manualRunner.trim()) ? "Saisissez un identifiant valide." : undefined}
                        onClick={() => {
                          setForm({ ...form, runners: [...new Set([...form.runners, manualRunner.trim()])] });
                          setManualRunner("");
                        }}
                      >
                        Ajouter
                      </Button>
                    </div>
                  </details>
                </fieldset>
                <TextField
                  label="Âge maximal de la preuve (heures)"
                  type="number"
                  min={0.02}
                  max={720}
                  step="any"
                  value={form.proofHours}
                  onChange={(event) => setForm({ ...form, proofHours: Number(event.target.value) })}
                  disabled={Boolean(blocked)}
                />
              </>
            )}
          </Card>
          <div className="team-actions">
            <Button onClick={() => setForm(baseline)} disabled={!dirty}>
              Annuler les modifications
            </Button>
            <Button variant="primary" disabledReason={blocked ?? invalid} onClick={save}>
              Enregistrer la protection
            </Button>
          </div>
        </>
      )}
      <ConfirmDialog value={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
