import { useState, type FormEvent } from "react";
import { Banner, Button, Card, PageHeader, TextField, navigate } from "@layrs/ui";
import { useProjectContexts } from "../app/contexts";
import { describeError, useSession } from "../app/session";

const validSlug = /^[a-z0-9][a-z0-9-]{0,62}$/;
function slugFromName(name: string) {
  return name.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 63).replace(/-+$/g, "");
}

export function NewOrganisationPage() {
  const { api } = useSession();
  const { data, refresh } = useProjectContexts();
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [edited, setEdited] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const allowed = data?.multiOrganisationEnabled && data.canCreateOrganisation;
  async function create(event: FormEvent) {
    event.preventDefault();
    if (!allowed || busy || !name.trim() || !validSlug.test(slug)) return;
    setBusy(true);
    setError("");
    try {
      const result = await api<{ id?: string; workspaceId?: string }>("/v3/organisations", "POST", { slug, name: name.trim() });
      await refresh(result.workspaceId ?? result.id);
      navigate("/projects");
    } catch (cause) {
      setError(describeError(cause));
    } finally { setBusy(false); }
  }
  return <div className="layrs-page layrs-page--narrow">
    <PageHeader title="Nouvelle organisation" description="Créez un espace distinct avec ses propres membres, équipes et projets." />
    {!allowed ? <Banner tone="warning">La création d’organisations n’est pas activée ou votre compte n’y est pas autorisé.</Banner> : <Card>
      <form className="new-project-form" onSubmit={(event) => void create(event)}>
        <TextField label="Nom de l’organisation" value={name} maxLength={255} onChange={(event) => { setName(event.target.value); if (!edited) setSlug(slugFromName(event.target.value)); }} />
        <TextField label="Identifiant" value={slug} maxLength={63} hint="Lettres minuscules, chiffres et tirets." onChange={(event) => { setEdited(true); setSlug(event.target.value); }} />
        {error && <Banner tone="danger" title="Création impossible">{error}</Banner>}
        <div className="new-project-form__actions">
          <Button type="submit" busy={busy} disabledReason={!name.trim() || !validSlug.test(slug) ? "Complétez les champs." : undefined}>Créer l’organisation</Button>
          <Button variant="ghost" onClick={() => navigate("/projects")}>Annuler</Button>
        </div>
      </form>
    </Card>}
  </div>;
}
