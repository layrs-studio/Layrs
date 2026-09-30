import { useEffect, useState, type ReactNode } from "react";
import { Button } from "../components/Button";
import { Badge, Banner, Card, TechnicalId } from "../components/Feedback";
import { SelectField, TextAreaField, TextField } from "../components/Field";
import { PolicyChangeDialog, type PreparedPolicyChange } from "./PolicyChangeDialog";
import type { PolicyPreview, PolicyTransport } from "./types";
import type { Directory } from "./useDirectory";
import { useTask } from "./useTask";

const HASH = /^blake3:[0-9a-f]{64}$/;

/**
 * P17 · Avancé: technical references and rare policy operations (return to
 * team-managed rights, JSON rules, stored policy by reference). Every change
 * is a server-bound preview followed by an explicit confirmation.
 */
export function AdvancedPanel({
  scope,
  spaceId,
  directory,
  policy,
  canManagePolicy,
  projectName,
  onChanged,
  desktopNote,
  onDirtyChange,
}: {
  scope: string;
  spaceId: string;
  directory: Directory;
  policy?: PolicyTransport;
  /** Capability hint `managePolicy`; null when unknown. */
  canManagePolicy: boolean | null;
  projectName: string;
  onChanged?: () => void;
  /** Explains functions that live elsewhere (e.g. Git import on Desktop). */
  desktopNote?: ReactNode;
  /** A JSON document or reference typed but not applied. */
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const task = useTask(scope);
  const [prepared, setPrepared] = useState<PreparedPolicyChange | null>(null);
  const [target, setTarget] = useState("");
  const [policyId, setPolicyId] = useState("");
  const [rules, setRules] = useState("");
  const { management, rules: read } = directory;
  const dirty = Boolean(target.trim()) || (Boolean(rules.trim()) && rules !== JSON.stringify(read?.policies.find((item) => item.policyId === policyId)?.rules ?? null, null, 2));
  useEffect(() => onDirtyChange?.(dirty), [dirty, onDirtyChange]);
  useEffect(() => () => onDirtyChange?.(false), [onDirtyChange]);
  const blocked = !policy
    ? "Les changements de politique ne sont pas disponibles dans cette application."
    : canManagePolicy === false || !directory.canAdminister
      ? "Réservé aux administrateurs du projet."
      : undefined;

  async function prepare(label: string, command: Record<string, unknown>, effects: ReactNode[], confirmLabel: string, danger = false) {
    if (!policy) return;
    await task.run("prepare", async (live) => {
      const { preview } = await policy<{ preview: PolicyPreview }>(command as { action: string });
      if (!live()) return;
      if (read && (preview.expectedPolicySetId !== read.policySetId || preview.expectedAuthorizationEpoch !== read.authorizationEpoch))
        throw new Error("La politique a changé depuis la lecture. Rechargez la page avant de préparer ce changement.");
      setPrepared({
        title: label,
        subject: `Projet ${projectName}`,
        effects: [...effects, "Ce changement peut retirer votre propre accès. Le serveur le refusera si la politique a changé entre-temps."],
        preview,
        confirmLabel,
        successTitle: "Politique appliquée",
        danger,
      });
    }, "L’aperçu n’a pas pu être préparé.");
  }

  let parsed: unknown = null;
  let rulesError: string | undefined;
  if (rules.trim()) {
    try {
      parsed = JSON.parse(rules);
      if (new TextEncoder().encode(rules).length > 64 * 1024) rulesError = "Le document dépasse 64 Kio.";
    } catch {
      rulesError = "JSON invalide.";
    }
  }

  return (
    <div className="team-panel team-panel--narrow">
      <Card className="settings-card">
        <h3 className="team-section-title">Références techniques</h3>
        <dl className="settings-facts">
          <dt>Projet</dt>
          <dd>
            <TechnicalId value={spaceId} label="Copier la référence" />
          </dd>
          {management && (
            <>
              <dt>Gestion des droits</dt>
              <dd>
                <Badge tone={management.mode === "managed" ? "success" : "warning"}>{management.mode === "managed" ? "Équipes et rôles" : "Règles personnalisées"}</Badge>
              </dd>
              <dt>Politique actuelle</dt>
              <dd>
                <TechnicalId value={management.currentPolicySetId} label="Copier" />
              </dd>
              <dt>Politique gérée</dt>
              <dd>
                <TechnicalId value={management.managedPolicySetId} label="Copier" />
              </dd>
            </>
          )}
        </dl>
        {desktopNote}
      </Card>

      {task.error && <Banner tone="danger">{task.error}</Banner>}
      {blocked ? (
        <Banner tone="info">{blocked}</Banner>
      ) : (
        <>
          {management?.mode === "custom" && (
            <Card className="settings-card">
              <h3 className="team-section-title">Revenir à la gestion par équipes et rôles</h3>
              <p className="team-muted">
                Rétablit la dernière politique gérée par les équipes, rôles et invitations. Les règles personnalisées ajoutées depuis, y compris les règles de fichiers, sont
                retirées.
              </p>
              <div>
                <Button
                  busy={task.pending === "prepare"}
                  busyLabel="Préparation…"
                  onClick={() =>
                    void prepare(
                      "Revenir à la gestion par équipes",
                      { action: "previewPolicy", policySetId: management.managedPolicySetId },
                      [
                        "La politique gérée précédente redevient active.",
                        "Toutes les règles personnalisées et règles de fichiers ajoutées depuis sont retirées.",
                        "Les rôles, équipes et invitations redeviennent modifiables.",
                      ],
                      "Rétablir la gestion par équipes",
                      true,
                    )
                  }
                >
                  Préparer le retour
                </Button>
              </div>
            </Card>
          )}
          <Card className="settings-card">
            <h3 className="team-section-title">Règles d’accès en JSON</h3>
            <p className="team-muted">
              Pour les cas que les pages Membres et Accès aux fichiers ne couvrent pas. Documents de règles v1, v2 ou v3 ; un refus est toujours prioritaire. Toute règle
              ajoutée ici fait passer le projet en règles personnalisées.
            </p>
            <SelectField
              label="Politique concernée"
              value={policyId}
              onChange={(event) => {
                const id = event.target.value;
                setPolicyId(id);
                const selected = read?.policies.find((item) => item.policyId === id);
                setRules(selected ? JSON.stringify(selected.rules, null, 2) : "");
              }}
              hint={read ? "Choisir une politique existante recopie ses règles pour les remplacer entièrement." : "Les règles actuelles n’ont pas pu être lues : seul l’ajout est possible."}
            >
              <option value="">Ajouter une nouvelle politique</option>
              {(directory.policies ?? []).map((item) => (
                <option key={item.policyId} value={item.policyId}>
                  {item.label} · {item.policyId.slice(7, 15)}
                </option>
              ))}
            </SelectField>
            <TextAreaField label="Document JSON des règles" rows={10} spellCheck={false} value={rules} error={rulesError} onChange={(event) => setRules(event.target.value)} />
            <div>
              <Button
                busy={task.pending === "prepare"}
                busyLabel="Préparation…"
                disabledReason={!rules.trim() ? "Saisissez un document de règles." : rulesError}
                onClick={() =>
                  void prepare(
                    policyId ? "Remplacer les règles d’une politique" : "Ajouter une politique",
                    policyId ? { action: "previewReplacePolicyRules", policyId, rules: parsed } : { action: "previewPolicyRules", rules: parsed },
                    [
                      policyId
                        ? "Les règles de cette seule politique sont remplacées ; les autres politiques sont conservées et l’ancienne version reste dans l’historique."
                        : "Une politique indépendante est ajoutée aux politiques actuelles.",
                      ...(management?.mode === "managed" ? ["Le projet passera en règles personnalisées : rôles, équipes et invitations seront suspendus."] : []),
                    ],
                    policyId ? "Remplacer les règles" : "Ajouter la politique",
                  )
                }
              >
                Prévisualiser
              </Button>
            </div>
          </Card>
          <Card className="settings-card">
            <h3 className="team-section-title">Appliquer une politique enregistrée</h3>
            <p className="team-muted">Pour revenir à une politique connue du serveur, identifiée par sa référence.</p>
            <TextField
              label="Référence de la politique"
              placeholder="blake3:…"
              value={target}
              spellCheck={false}
              error={target && !HASH.test(target.trim()) ? "Format attendu : blake3: suivi de 64 caractères hexadécimaux." : undefined}
              onChange={(event) => setTarget(event.target.value)}
            />
            <div>
              <Button
                busy={task.pending === "prepare"}
                busyLabel="Préparation…"
                disabledReason={!HASH.test(target.trim()) ? "Saisissez une référence valide." : undefined}
                onClick={() =>
                  void prepare(
                    "Appliquer une politique enregistrée",
                    { action: "previewPolicy", policySetId: target.trim() },
                    ["Toutes les règles actuelles sont remplacées par celles de la politique indiquée."],
                    "Appliquer cette politique",
                    true,
                  )
                }
              >
                Prévisualiser
              </Button>
            </div>
          </Card>
        </>
      )}
      {policy && (
        <PolicyChangeDialog
          value={prepared}
          policy={policy}
          onClose={() => setPrepared(null)}
          onApplied={async () => {
            setRules("");
            setPolicyId("");
            setTarget("");
            await directory.reload();
            onChanged?.();
          }}
        />
      )}
    </div>
  );
}
