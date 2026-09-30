import { useCallback, useMemo, useState, type ReactNode } from "react";
import { Settings2, ShieldCheck, SlidersHorizontal, Webhook, Workflow } from "lucide-react";
import {
  AdvancedPanel,
  AutomationsPanel,
  Badge,
  Card,
  Link,
  Loading,
  PageHeader,
  ProtectionsPanel,
  TechnicalId,
  WebhooksPanel,
  navigate,
  segment,
  useDirectory,
  useNavigationGuard,
  type LayerRef,
} from "@layrs/ui";
import { useTeamContext } from "./team-context";

type Section = "general" | "protections" | "automations" | "webhooks" | "advanced";
const sections: { id: Section; label: string; icon: ReactNode }[] = [
  { id: "general", label: "Général", icon: <Settings2 aria-hidden size={16} /> },
  { id: "protections", label: "Protections", icon: <ShieldCheck aria-hidden size={16} /> },
  { id: "automations", label: "Automatisations", icon: <Workflow aria-hidden size={16} /> },
  { id: "webhooks", label: "Webhooks", icon: <Webhook aria-hidden size={16} /> },
  { id: "advanced", label: "Avancé", icon: <SlidersHorizontal aria-hidden size={16} /> },
];

/** P17 · Réglages du projet (Web parts). Sub-pages are routes: /settings/<section>. */
export function ProjectSettingsPage({ section, layer }: { section?: string; layer?: string }) {
  const context = useTeamContext();
  const current: Section = sections.some((item) => item.id === section) ? (section as Section) : "general";
  const [dirty, setDirty] = useState(false);
  useNavigationGuard(dirty ? "Des modifications ne sont pas enregistrées. Quitter cette page et les perdre ?" : null);
  const key = segment(context.project.spaceId);
  return (
    <div className="layrs-page">
      <PageHeader title="Réglages du projet" description="Règles partagées du projet : protections, automatisations et intégrations. Chaque changement est enregistré explicitement." />
      <div className="team-settings">
        <nav className="team-settings__nav" aria-label="Sections des réglages">
          <ul>
            {sections.map((item) => (
              <li key={item.id}>
                <Link to={`/projects/${key}/settings/${item.id}`} aria-current={item.id === current ? "page" : undefined}>
                  {item.icon}
                  {item.label}
                  {item.id === current && dirty && <small>Non enregistré</small>}
                </Link>
              </li>
            ))}
          </ul>
        </nav>
        <div>
          {context.capabilities.loading && !context.capabilities.value ? (
            <Loading label="Lecture de vos droits sur le projet…" />
          ) : (
            <SettingsContent section={current} layer={layer} context={context} onDirtyChange={setDirty} />
          )}
        </div>
      </div>
    </div>
  );
}

function SettingsContent({
  section,
  layer,
  context,
  onDirtyChange,
}: {
  section: Section;
  layer?: string;
  context: ReturnType<typeof useTeamContext>;
  onDirtyChange: (dirty: boolean) => void;
}) {
  const { project, scope, capabilities, policy, self, instanceAdmin } = context;
  const flags = capabilities.flags;
  const directory = useDirectory({ scope, administration: project.administration, policy, instanceAdmin, self, administer: flags ? flags.managePolicy : null });
  const layers: LayerRef[] = useMemo(
    () => (project.bootstrap?.layers ?? []).map((item) => ({ layerId: item.record.layerId, name: item.record.name, headStepIds: item.headStepIds })),
    [project.bootstrap],
  );
  const key = segment(project.spaceId);
  const changed = useCallback(() => void capabilities.reload(), [capabilities]);
  const dirty = useCallback((value: boolean) => onDirtyChange(value), [onDirtyChange]);

  switch (section) {
    case "protections":
      return (
        <ProtectionsPanel
          scope={scope}
          directory={directory}
          administration={project.administration}
          layers={layers}
          projectName={project.name}
          canConfigure={flags ? flags.configureProtection : null}
          initialLayer={layer}
          onDirtyChange={dirty}
        />
      );
    case "automations":
      return (
        <AutomationsPanel
          scope={scope}
          directory={directory}
          administration={project.administration}
          layers={layers}
          onEdit={(layerId) => navigate(`/projects/${key}/settings/protections`, { query: { layer: layerId } })}
          onOpenRuns={() => navigate(`/projects/${key}/reviews`)}
        />
      );
    case "webhooks":
      return <WebhooksPanel scope={scope} administration={project.administration} projectName={project.name} canManage={flags ? flags.manageWebhooks : null} />;
    case "advanced":
      return (
        <AdvancedPanel
          scope={scope}
          spaceId={project.spaceId}
          directory={directory}
          policy={policy}
          canManagePolicy={flags ? flags.managePolicy : null}
          projectName={project.name}
          onChanged={changed}
          onDirtyChange={dirty}
          desktopNote={
            <p className="team-muted team-small">
              L’import/export Git et la migration d’un dossier vers un accès limité se font depuis Layrs Desktop, qui travaille sur les fichiers locaux.
            </p>
          }
        />
      );
    default:
      return <General context={context} layers={layers} managed={directory.management?.mode} />;
  }
}

function General({ context, layers, managed }: { context: ReturnType<typeof useTeamContext>; layers: LayerRef[]; managed?: "managed" | "custom" }) {
  const { project, capabilities } = context;
  const flags = capabilities.flags;
  const key = segment(project.spaceId);
  const can = flags
    ? [
        flags.applyChanges && "publier",
        flags.proposeReviews && "proposer des revues",
        flags.decideReviews && "approuver",
        flags.publishReleases && "publier des versions",
        flags.manageMembers && "gérer les membres et équipes",
        flags.managePolicy && "régler les accès",
        flags.configureProtection && "régler les protections",
        flags.manageWebhooks && "gérer les webhooks",
      ].filter(Boolean)
    : [];
  return (
    <div className="team-panel team-panel--narrow">
      <Card className="settings-card">
        <h2 className="team-section-title">Renseignements</h2>
        <dl className="settings-facts">
          <dt>Nom</dt>
          <dd>{project.name}</dd>
          <dt>Référence technique</dt>
          <dd>
            <TechnicalId value={project.spaceId} label="Copier la référence" />
          </dd>
          <dt>Layers publiés</dt>
          <dd>{layers.length ? layers.map((layer) => layer.name).join(", ") : "Aucun"}</dd>
          {managed && (
            <>
              <dt>Gestion des droits</dt>
              <dd>
                <Badge tone={managed === "managed" ? "success" : "warning"}>{managed === "managed" ? "Équipes et rôles" : "Règles personnalisées"}</Badge>
              </dd>
            </>
          )}
          {capabilities.value && (
            <>
              <dt>Votre accès</dt>
              <dd>
                {capabilities.value.access === "limited" ? "Accès limité : certains contenus sont protégés. " : ""}
                {can.length ? `Vous pouvez ${can.join(", ")}.` : "Consultation."}
              </dd>
            </>
          )}
        </dl>
        <p className="team-muted team-small">
          Le nom et l’organisation du projet ne se modifient pas depuis Layrs pour l’instant. Les personnes et leurs droits se règlent dans{" "}
          <Link to={`/projects/${key}/team`}>Équipe et accès</Link> et <Link to={`/projects/${key}/access`}>Accès aux fichiers</Link>.
        </p>
      </Card>
    </div>
  );
}
