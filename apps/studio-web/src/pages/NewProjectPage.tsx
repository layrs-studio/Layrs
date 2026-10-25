import { useEffect, useState, type FormEvent } from "react";
import { Banner, Button, Card, Loading, PageHeader, TextField, navigate } from "@layrs/ui";
import { rememberProject } from "../app/project";
import { useProjectContexts } from "../app/contexts";
import { ApiError, describeError, useSession } from "../app/session";

type CreatedProject = { workspaceId: string; spaceId: string; layerId: string };
type LensPin = { lensId: string; version: string; digest: string };
type LensCatalogItem = { manifest: { id: string; name: string; analyzer: { fileExtensions: string[] } }; pin: LensPin; source: string; state: string; available: boolean };
type LensCatalog = { items: LensCatalogItem[] } | LensCatalogItem[];
const validSlug = /^[a-z0-9][a-z0-9-]{0,62}$/;

function slugFromName(name: string) {
  return name.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase()
    .replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 63).replace(/-+$/g, "");
}

export function NewProjectPage() {
  const { api } = useSession();
  const { selected } = useProjectContexts();
  const [loading, setLoading] = useState(true);
  const [loadingError, setLoadingError] = useState("");
  const [lenses, setLenses] = useState<LensCatalogItem[]>([]);
  const [selectedLenses, setSelectedLenses] = useState<Record<string, string>>({});
  const [extensionChoices, setExtensionChoices] = useState<Record<string, string>>({});
  const [name, setName] = useState("");
  const [spaceSlug, setSpaceSlug] = useState("");
  const [spaceSlugEdited, setSpaceSlugEdited] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    async function load() {
      try {
        const catalog = await api<LensCatalog>("/v3/lenses/catalog");
        if (!active) return;
        const available = (Array.isArray(catalog) ? catalog : catalog.items).filter((item) => item.available && item.state === "active");
        setLenses(available);
        const defaults: Record<string, string> = {};
        for (const item of available) {
          if (item.source === "builtin" && !defaults[item.pin.lensId]) defaults[item.pin.lensId] = item.pin.digest;
        }
        setSelectedLenses(defaults);
      } catch (cause) {
        if (active) setLoadingError(describeError(cause));
      } finally {
        if (active) setLoading(false);
      }
    }
    void load();
    return () => { active = false; };
  }, [api]);

  const projectName = name.trim();
  const lensGroups = new Map<string, LensCatalogItem[]>();
  for (const item of lenses) lensGroups.set(item.pin.lensId, [...(lensGroups.get(item.pin.lensId) ?? []), item]);
  const chosen = lenses.filter((item) => selectedLenses[item.pin.lensId] === item.pin.digest);
  const extensions = new Map<string, string[]>();
  for (const item of chosen) {
    for (const rawExtension of item.manifest.analyzer.fileExtensions) {
      const extension = rawExtension.startsWith(".") ? rawExtension : `.${rawExtension}`;
      const candidates = extensions.get(extension) ?? [];
      if (!candidates.includes(item.pin.lensId)) candidates.push(item.pin.lensId);
      extensions.set(extension, candidates);
    }
  }
  const invalid = !selected?.canCreateProject || !projectName || projectName.length > 255 || !validSlug.test(spaceSlug);

  async function create(event: FormEvent) {
    event.preventDefault();
    if (invalid || busy) return;
    setBusy(true);
    setError("");
    try {
      const request = { contextId: selected!.id, spaceSlug, name: projectName };
      const lensConfiguration = {
        pins: chosen.map((item) => item.pin),
        associations: [...extensions].sort(([a], [b]) => a.localeCompare(b)).map(([extension, candidates]) => ({
          extension, lensId: candidates.includes(extensionChoices[extension]) ? extensionChoices[extension] : candidates[0],
        })),
      };
      const result = await api<CreatedProject>("/v3/projects", "POST", { ...request, lensConfiguration });
      rememberProject(result.spaceId, projectName, selected!.id);
      navigate(`/projects/${encodeURIComponent(result.spaceId)}/files`);
    } catch (cause) {
      setError(cause instanceof ApiError && cause.code === "invalid_descriptor"
        ? "Cet identifiant est déjà utilisé ou les paramètres diffèrent d’un projet existant. Vérifiez les noms et réessayez."
        : describeError(cause));
    } finally {
      setBusy(false);
    }
  }

  return <div className="layrs-page">
    <PageHeader title="Nouveau projet" description="Créez un projet sur cette instance, puis connectez un dossier depuis Desktop pour publier vos fichiers." />
    {loading ? <Loading label="Chargement des lenses…" /> : <>
      {loadingError && <Banner tone="danger" title="Lenses indisponibles">{loadingError}</Banner>}
      {!loadingError && <Card><form className="new-project-form" onSubmit={(event) => void create(event)}>
        <p>Dans <strong>{selected?.kind === "personal" ? "votre espace personnel" : selected?.name}</strong>.</p>
        {!selected?.canCreateProject && <Banner tone="warning">Votre compte ne peut pas créer de projet dans cet espace.</Banner>}
        <TextField label="Nom du projet" value={name} maxLength={255} onChange={(event) => {
          const value = event.target.value;
          setName(value);
          if (!spaceSlugEdited) setSpaceSlug(slugFromName(value));
        }} />
        <TextField label="Identifiant du projet" value={spaceSlug} maxLength={63} hint="Unique dans cet espace ; lettres minuscules, chiffres et tirets." onChange={(event) => { setSpaceSlugEdited(true); setSpaceSlug(event.target.value); }} />
        <fieldset className="new-project-form__lenses"><legend>Lenses du projet</legend>
          {lensGroups.size === 0 ? <p>Aucune lens installée n’est disponible.</p> : [...lensGroups].map(([lensId, versions]) => <label className="new-project-form__field" key={lensId}>
            {versions[0].manifest.name}
            <select value={selectedLenses[lensId] ?? ""} onChange={(event) => setSelectedLenses((current) => ({ ...current, [lensId]: event.target.value }))}>
              <option value="">Non activée</option>
              {versions.map((item) => <option key={`${item.pin.version}:${item.pin.digest}`} value={item.pin.digest}>Version {item.pin.version}</option>)}
            </select>
          </label>)}
        </fieldset>
        {extensions.size > 0 && <details><summary>Associations aux fichiers ({extensions.size})</summary><fieldset className="new-project-form__lenses"><legend>Associations aux fichiers</legend>
          {[...extensions].sort(([a], [b]) => a.localeCompare(b)).map(([extension, candidates]) => <label className="new-project-form__field" key={extension}>{extension}
            <select value={candidates.includes(extensionChoices[extension]) ? extensionChoices[extension] : candidates[0]} onChange={(event) => setExtensionChoices((current) => ({ ...current, [extension]: event.target.value }))}>
              {candidates.map((lensId) => <option key={lensId} value={lensId}>{lenses.find((item) => item.pin.lensId === lensId)?.manifest.name ?? lensId}</option>)}
            </select>
          </label>)}
        </fieldset></details>}
        {error && <Banner tone="danger" title="Création impossible">{error}</Banner>}
        <div className="new-project-form__actions">
          <Button type="submit" busy={busy} busyLabel="Création…" disabledReason={invalid ? "Complétez les champs et les identifiants." : undefined}>Créer le projet</Button>
          <Button variant="ghost" onClick={() => navigate("/projects")}>Annuler</Button>
        </div>
      </form></Card>}
    </>}
  </div>;
}
