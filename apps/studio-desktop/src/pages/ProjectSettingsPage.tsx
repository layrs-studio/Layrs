import { useState } from "react";
import { Button, Card, PageHeader, SelectField, TechnicalId, TextField } from "@layrs/ui";
import { NativeIdentityPanel } from "../NativeIdentityPanel";
import { useWorkspace } from "../app/workspace";

export function ProjectSettingsPage() {
  const workspace = useWorkspace();
  const { data, busy } = workspace;
  const locked = Boolean(busy) || Boolean(data?.needsRepair);
  const [rewriteKind, setRewriteKind] = useState("replay");
  const [rewriteSource, setRewriteSource] = useState("");
  const [mainline, setMainline] = useState("");

  function previewRewrite() {
    const params = { kind: rewriteKind, source: rewriteSource, mainline: rewriteKind === "squash" ? null : mainline || null };
    void workspace.inspect({ action: "previewRewrite", ...params }, { action: "applyRewrite", ...params }, "Transformer des changements");
  }

  return (
    <div className="layrs-page layrs-page--narrow">
      <PageHeader title="Réglages du projet" description="Préférences de ce dossier sur cet ordinateur. Elles ne modifient pas les règles de l’équipe." />

      <section className="settings-section" aria-labelledby="settings-general">
        <h2 id="settings-general">Général</h2>
        <Card className="settings-card">
          <dl className="settings-facts">
            <dt>Nom</dt>
            <dd>{data?.state.space.name ?? "—"}</dd>
            <dt>Dossier</dt>
            <dd>
              <code>{workspace.root}</code>
            </dd>
            <dt>Référence technique</dt>
            <dd>{data ? <TechnicalId value={data.state.space.spaceId} /> : "—"}</dd>
          </dl>
        </Card>
      </section>

      <section className="settings-section" aria-labelledby="settings-identity">
        <h2 id="settings-identity">Identité des prochaines captures</h2>
        <p className="settings-section__hint">
          Nom déclaré pour les captures futures de ce poste. Il ne réécrit pas les anciennes captures et n’accorde aucun droit sur le serveur.
        </p>
        <div className="layrs-legacy">
          <NativeIdentityPanel key={workspace.root} root={workspace.root} disabled={locked} />
        </div>
      </section>

      <section className="settings-section" aria-labelledby="settings-advanced">
        <h2 id="settings-advanced">Avancé : transformer des changements</h2>
        <p className="settings-section__hint">
          Rejouer, inverser ou regrouper des captures. Un aperçu montre l’impact sur les Layers et points nommés avant toute écriture.
        </p>
        <Card className="settings-card">
          <SelectField label="Action" value={rewriteKind} onChange={(event) => setRewriteKind(event.target.value)}>
            <option value="replay">Rejouer un changement</option>
            <option value="revert">Inverser un changement</option>
            <option value="squash">Regrouper jusqu’à la capture courante</option>
          </SelectField>
          <TextField label="Capture ou référence" value={rewriteSource} placeholder="blake3:… ou @marker:version" onChange={(event) => setRewriteSource(event.target.value)} />
          {rewriteKind !== "squash" && (
            <TextField
              label="Parent de référence si fusion"
              value={mainline}
              placeholder="Obligatoire pour une capture à plusieurs parents"
              onChange={(event) => setMainline(event.target.value)}
            />
          )}
          <div>
            <Button disabledReason={locked ? "Opération en cours" : !rewriteSource ? "Indiquez la capture concernée." : undefined} onClick={previewRewrite}>
              Prévisualiser la transformation
            </Button>
          </div>
        </Card>
      </section>
    </div>
  );
}
