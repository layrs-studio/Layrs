import { useState, type FormEvent } from "react";
import { Banner, Button, Card, PageHeader, TextField, navigate } from "@layrs/ui";
import { useDesktopContexts } from "../app/contexts";
import { useRemote } from "../app/remote";
import { explainError } from "../app/errors";

const validSlug = /^[a-z0-9][a-z0-9-]{0,62}$/;
function slugFromName(name: string) {
  return name.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 63).replace(/-+$/g, "");
}

export function NewOrganisationPage() {
  const remote = useRemote();
  const contexts = useDesktopContexts();
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [edited, setEdited] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const allowed = contexts.data?.multiOrganisationEnabled && contexts.data.canCreateOrganisation && remote.session.state === "connected";
  async function create(event: FormEvent) {
    event.preventDefault();
    if (!allowed || busy || !validSlug.test(slug) || !name.trim()) return;
    setBusy(true);
    setError("");
    try {
      const result = await remote.transport<{ workspaceId?: string; id?: string }>({ action: "createOrganisation", request: { slug, name: name.trim() } }, { root: remote.root, origin: remote.origin, principalId: contexts.principalId });
      await contexts.refresh(result.workspaceId ?? result.id);
      navigate("/projects");
    } catch (cause) { setError(explainError(cause, "remote").detail); }
    finally { setBusy(false); }
  }
  return <div className="layrs-page layrs-page--narrow">
    <PageHeader title="Nouvelle organisation" description="Créez un espace distinct avec ses propres membres, équipes et projets." />
    {!allowed ? <Banner tone="warning">Connectez-vous avec un compte autorisé sur un serveur où plusieurs organisations sont activées.</Banner> : <Card>
      <form className="new-project-form" onSubmit={(event) => void create(event)}>
        <TextField label="Nom de l’organisation" value={name} maxLength={255} onChange={(event) => { setName(event.target.value); if (!edited) setSlug(slugFromName(event.target.value)); }} />
        <TextField label="Identifiant" value={slug} maxLength={63} hint="Lettres minuscules, chiffres et tirets." onChange={(event) => { setEdited(true); setSlug(event.target.value); }} />
        {error && <Banner tone="danger" title="Création impossible">{error}</Banner>}
        <div className="new-project-form__actions"><Button type="submit" busy={busy} disabledReason={!name.trim() || !validSlug.test(slug) ? "Complétez les champs." : undefined}>Créer l’organisation</Button><Button variant="ghost" onClick={() => navigate("/projects")}>Annuler</Button></div>
      </form>
    </Card>}
  </div>;
}
