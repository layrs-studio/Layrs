import { useCallback, useMemo, useState, type ReactNode } from "react";
import { Send, Shield, Users } from "lucide-react";
import {
  AccessPanel,
  AccountsPanel,
  AdvancedPanel,
  InstalledLensesPanel,
  OrganisationSettingsPanel,
  ProjectLensesPanel,
  AutomationsPanel,
  Badge,
  Button,
  Card,
  CaptureDetail,
  EmptyState,
  InvitationInbox,
  InvitationsPanel,
  Loading,
  MembersPanel,
  PageHeader,
  ProjectActivity,
  ProjectFiles,
  ProjectHistory,
  ProtectionsPanel,
  Releases,
  ReviewDetail,
  ReviewList,
  RunDetail,
  ServerStatusPanel,
  ServiceAccountsPanel,
  StoragePanel,
  Tabs,
  TeamsPanel,
  TokensPanel,
  WebhooksPanel,
  navigate,
  segment,
  useDirectory,
  useLocation,
  useNavigationGuard,
  type HistoryFilters,
  type LayerRef,
  type PolicyPreview,
  type ReviewFilter,
  type ReviewTab,
  type ReleasesTab,
} from "@layrs/ui";
import { NativeScopedReviews } from "../NativeScopedReviews";
import { useProjection } from "../app/projection";
import { useRemote } from "../app/remote";
import { useWorkspace } from "../app/workspace";
import { ServerSummary } from "../sync/parts";
import { useRemoteTeam, type RemoteTeam } from "./remoteTeam";

const reviewFilters: ReviewFilter[] = ["open", "integrated", "closed", "all"];
const reviewTabs: ReviewTab[] = ["discussion", "files", "validations"];
const historyKeys = ["from", "path", "since", "until", "actor"] as const;
const adminSections = ["accounts", "services", "storage", "status", "lenses", "organisations"] as const;
const settingsSections = ["general", "protections", "automations", "webhooks", "advanced", "lenses"] as const;
type PageSection = "team" | "reviews" | "published" | "access" | "admin" | "remote-settings";

function base(root: string) {
  return `/projects/${segment(root)}`;
}

function historyFilters(query: URLSearchParams): HistoryFilters {
  return { from: query.get("from") ?? "", path: query.get("path") ?? "", since: query.get("since") ?? "", until: query.get("until") ?? "", actor: query.get("actor") ?? "" };
}

function historyQuery(filters: HistoryFilters) {
  return Object.fromEntries(historyKeys.map((key) => [key, filters[key] || undefined]));
}

/** Project-level remote surfaces, kept separate from the local project views. */
export function TeamPage({ section = "team", detail = "" }: { section?: string; detail?: string }) {
  const workspace = useWorkspace();
  const remote = useRemote();
  if (remote.session.state !== "connected")
    return (
      <div className="layrs-page">
        <PageHeader title="Équipe" description="Collaboration et administration sur le serveur de votre équipe." />
        <Card className="team-connect">
          <EmptyState icon={<Users size={22} />} title="Connectez ce projet au serveur de votre équipe">
            Les revues et l’administration exigent une session sur le serveur. Vos captures locales restent utilisables sans connexion.
          </EmptyState>
          <ServerSummary />
        </Card>
      </div>
    );
  if (workspace.kind === "projection") return <LimitedTeam />;
  return <CompleteTeam section={section as PageSection} detail={detail} />;
}

function CompleteTeam({ section, detail }: { section: PageSection; detail: string }) {
  const context = useRemoteTeam();
  const workspace = useWorkspace();
  if (section === "admin") {
    if (!context.who)
      return context.whoError ? <div className="layrs-page"><PageHeader title="Administration de l’instance" />
        <EmptyState title="Identité non vérifiée">{context.whoError}</EmptyState></div>
        : <div className="layrs-page"><Loading label="Vérification de l’administrateur…" /></div>;
    return <Admin context={context} detail={detail} />;
  }
  if (context.error)
    return <div className="layrs-page"><PageHeader title="Projet distant" /><EmptyState title="Le projet distant n’a pas pu être lu">
      {context.error}
      {context.who?.instance_administrator && <p><Button size="sm" onClick={() => navigate(`${base(workspace.root)}/admin`)}>Administration de l’instance</Button></p>}
    </EmptyState></div>;
  if (!context.bootstrap || !context.who) return <div className="layrs-page"><Loading label="Lecture du projet distant…" /></div>;
  // A full local checkout can outlive a restriction on the server. Never ask
  // whole-project collaboration endpoints after the capability turns limited.
  if (context.capabilities.value?.access === "limited" || context.references.restricted)
    return (
      <div className="layrs-page">
        <PageHeader title="Accès limité" />
        <EmptyState icon={<Shield size={22} />} title="Les contenus complets ne sont plus accessibles">
          Actualisez la connexion et utilisez un dossier à accès limité pour les chemins autorisés.
        </EmptyState>
      </div>
    );
  switch (section) {
    case "reviews": return <Reviews context={context} detail={detail} />;
    case "published": return <Published context={context} detail={detail} />;
    case "access": return <Access context={context} />;
    case "remote-settings": return <RemoteSettings context={context} detail={detail} />;
    default: return <Team context={context} detail={detail} isAdmin={context.who.instance_administrator} />;
  }
}

function Team({ context, detail, isAdmin }: { context: RemoteTeam; detail: string; isAdmin: boolean }) {
  const workspace = useWorkspace();
  const remote = useRemote();
  const path = base(workspace.root);
  const current = detail === "invitations" || detail === "teams" || detail === "inbox" || detail === "activity" || detail === "tokens" ? detail : "members";
  const [inviteRequested, setInviteRequested] = useState(false);
  const directory = useDirectory({
    scope: context.scope,
    administration: context.administration,
    policy: context.policy,
    instanceAdmin: isAdmin,
    self: context.self,
    administer: context.capabilities.flags?.managePolicy ?? null,
  });
  const changed = useCallback(() => void context.capabilities.reload(), [context.capabilities.reload]);
  const go = (tab: string) => navigate(`${path}/team${tab === "members" ? "" : `/${tab}`}`);
  return (
    <div className="layrs-page">
      <PageHeader title="Équipe et accès" description="Personnes, invitations, équipes et activité du projet."
        aside={context.capabilities.flags && <Badge tone={context.capabilities.flags.manageMembers ? "accent" : "info"}>{context.capabilities.flags.manageMembers ? "Administration du projet" : "Consultation"}</Badge>} />
      {isAdmin && <p><Button size="sm" onClick={() => navigate(`${path}/admin`)}>Administration de l’instance</Button></p>}
      {context.capabilities.loading && !context.capabilities.value ? <Loading label="Lecture de vos droits…" /> : (
        <Tabs label="Équipe et accès" value={current} onChange={go} items={[
          { id: "members", label: "Membres" },
          { id: "invitations", label: "Invitations" },
          { id: "teams", label: "Équipes" },
          { id: "inbox", label: "Mes invitations" },
          { id: "activity", label: "Activité" },
          { id: "tokens", label: "Mes jetons" },
        ]}>
          {current === "members" && <MembersPanel directory={directory} administration={context.administration} projectName={context.projectName} self={context.self}
            onChanged={changed} onInvite={() => { setInviteRequested(true); go("invitations"); }} />}
          {current === "invitations" && <InvitationsPanel scope={context.scope} directory={directory} administration={context.administration} projectName={context.projectName}
            self={context.self} inviteRequested={inviteRequested} onInviteHandled={() => setInviteRequested(false)} />}
          {current === "teams" && <TeamsPanel scope={context.scope} directory={directory} administration={context.administration} projectName={context.projectName} onChanged={changed} />}
          {current === "inbox" && <InvitationInbox scope={context.scope} administration={context.administration}
            projectName={(space) => space === context.spaceId ? context.projectName : undefined}
            onOpenProject={() => navigate("/projects")} />}
          {current === "activity" && <ProjectActivity scope={context.scope} administration={context.administration} projectName={context.projectName}
            onOpenReview={(review) => navigate(`${path}/reviews/${segment(review.reviewId)}`, { query: { version: String(review.version) } })} />}
          {current === "tokens" && <TokensPanel scope={context.scope} administration={context.administration} onCurrentRevoked={() => void remote.check()} />}
        </Tabs>
      )}
    </div>
  );
}

function Reviews({ context, detail }: { context: RemoteTeam; detail: string }) {
  const workspace = useWorkspace();
  const remote = useRemote();
  const path = `${base(workspace.root)}/reviews`;
  const { query } = useLocation();
  const [reviewId, subview, runId] = detail.split("/").map((part) => part ? decodeURIComponent(part) : "");
  const requested = Number(query.get("version"));
  const version = Number.isSafeInteger(requested) && requested > 0 ? requested : null;
  const tab = reviewTabs.includes(query.get("tab") as ReviewTab) ? query.get("tab") as ReviewTab : "discussion";
  const state = query.get("state");
  const filter = reviewFilters.includes(state as ReviewFilter) ? state as ReviewFilter : "open";
  const detailPath = `${path}/${segment(reviewId)}`;
  const queryFor = (next: { version?: number | null; tab?: ReviewTab }) => ({
    version: next.version === undefined ? version ? String(version) : undefined : next.version ? String(next.version) : undefined,
    tab: (next.tab ?? tab) === "discussion" ? undefined : next.tab ?? tab,
  });
  if (reviewId && subview === "runs" && runId)
    return <div className="layrs-page"><PageHeader title="Validation automatique" /><RunDetail key={`${context.scope}|${runId}`} scope={context.scope}
      transport={context.collaboration} runId={runId} references={context.references} people={context.people}
      onBack={() => navigate(detailPath, { query: queryFor({ tab: "validations" }) })} /></div>;
  if (reviewId)
    return <div className="layrs-page"><ReviewDetail key={`${context.scope}|${reviewId}`} scope={context.scope} transport={context.collaboration}
      spaceId={context.spaceId} reviewId={reviewId} version={version} references={context.references} people={context.people}
      draftIdentity={{ origin: remote.origin, principalId: context.people.currentPrincipalId! }} capabilities={context.flags ?? undefined}
      tab={tab} onTabChange={(next) => navigate(detailPath, { replace: true, query: queryFor({ tab: next }) })}
      onOpenVersion={(next) => navigate(detailPath, { query: queryFor({ version: next }) })}
      onOpenRun={(run) => navigate(`${detailPath}/runs/${segment(run)}`, { query: { version: version ? String(version) : undefined } })}
      integrationUnavailableReason="Intégrez depuis Synchronisation après avoir sélectionné la capture et le Layer exacts."
      onBack={() => navigate(path)} />
    </div>;
  return <div className="layrs-page"><PageHeader title="Revues" description="Propositions, discussions et décisions sur des versions précises." />
    <ReviewList key={context.scope} scope={context.scope} transport={context.collaboration} spaceId={context.spaceId} references={context.references}
      people={context.people} draftIdentity={{ origin: remote.origin, principalId: context.people.currentPrincipalId! }}
      capabilities={context.flags ?? undefined} filter={filter}
      onFilterChange={(next) => navigate(path, { replace: true, query: { state: next === "open" ? undefined : next } })}
      onOpenReview={(id) => navigate(`${path}/${segment(id)}`)} /></div>;
}

function Published({ context, detail }: { context: RemoteTeam; detail: string }) {
  const workspace = useWorkspace();
  const path = `${base(workspace.root)}/published`;
  const [part, id] = detail.split("/").map((item) => item ? decodeURIComponent(item) : "");
  const current = part === "history" || part === "versions" ? part : "files";
  const { query } = useLocation();
  const filters = historyFilters(query);
  const tab: ReleasesTab = query.get("tab") === "markers" ? "markers" : "releases";
  return <div className="layrs-page">
    <PageHeader title="Projet publié" description="Fichiers, captures et versions disponibles sur le serveur." />
    <Tabs label="Projet publié" value={current} onChange={(next) => navigate(`${path}/${next}`)}
      items={[{ id: "files", label: "Fichiers" }, { id: "history", label: "Historique" }, { id: "versions", label: "Versions" }]}>
      {current === "files" && <ProjectFiles key={context.scope} scope={context.scope} capabilities={context.flags ?? undefined}
        loadContext={context.loadContext} loadPage={context.loadPage} previewTransport={context.administration}
        onOpenHistory={(file) => navigate(`${path}/history`, { query: { path: file } })} />}
      {current === "history" && (id ? <CaptureDetail key={`${context.scope}|${id}`} scope={context.scope} transport={context.collaboration}
        references={context.references} people={context.people} stepId={id}
        onBack={() => navigate(`${path}/history`, { query: historyQuery(filters) })} />
        : <ProjectHistory key={context.scope} scope={context.scope} transport={context.collaboration} references={context.references}
          people={context.people} filters={filters}
          onFiltersChange={(next) => navigate(`${path}/history`, { replace: true, query: historyQuery(next) })}
          onOpenCapture={(stepId) => navigate(`${path}/history/${segment(stepId)}`, { query: historyQuery(filters) })} />)}
      {current === "versions" && <Releases key={context.scope} scope={context.scope} transport={context.collaboration}
        references={context.references} people={context.people} capabilities={context.flags ?? undefined} tab={tab}
        onTabChange={(next) => navigate(`${path}/versions`, { replace: true, query: { tab: next === "releases" ? undefined : next } })} />}
    </Tabs>
  </div>;
}

function Access({ context }: { context: RemoteTeam }) {
  const directory = useDirectory({ scope: context.scope, administration: context.administration, policy: context.policy,
    instanceAdmin: context.who?.instance_administrator === true, self: context.self, administer: context.capabilities.flags?.managePolicy ?? null });
  return <div className="layrs-page"><PageHeader title="Accès aux fichiers" description="Droits de lecture et de modification pour chaque chemin." />
    {context.capabilities.loading && !context.capabilities.value ? <Loading label="Lecture de vos droits…" /> :
      <AccessPanel scope={context.scope} directory={directory} policy={context.policy} loadContext={context.loadContext} loadPage={context.loadPage}
        self={context.self} projectName={context.projectName} canChange={context.capabilities.flags?.managePolicy ?? null}
        recoverPendingPolicy={() => context.policy<{ preview: PolicyPreview | null }>({ action: "readPendingPolicy" }).then((result) => result.preview)}
        onChanged={() => void context.capabilities.reload()} />}
  </div>;
}

function Admin({ context, detail }: { context: RemoteTeam; detail: string }) {
  const workspace = useWorkspace();
  const path = `${base(workspace.root)}/admin`;
  if (!context.who?.instance_administrator) return <div className="layrs-page"><PageHeader title="Administration de l’instance" />
    <EmptyState icon={<Shield size={22} />} title="Réservé aux administrateurs de l’instance" /></div>;
  const current = adminSections.includes(detail as typeof adminSections[number]) ? detail as typeof adminSections[number] : "accounts";
  return <div className="layrs-page"><PageHeader title="Administration de l’instance" description="Comptes, stockage et état du serveur." />
    <Tabs label="Administration" value={current} onChange={(next) => navigate(`${path}/${next}`)}
      items={[{ id: "organisations", label: "Organisations" }, { id: "accounts", label: "Comptes" }, { id: "services", label: "Comptes de service" }, { id: "storage", label: "Stockage" }, { id: "status", label: "État du serveur" }, { id: "lenses", label: "Lenses" }]}>
      {current === "organisations" && <OrganisationSettingsPanel key={context.scope} scope={context.scope} administration={context.administration} />}
      {current === "lenses" && <InstalledLensesPanel key={context.scope} scope={context.scope} transport={context.administration} />}
      {current === "accounts" && <AccountsPanel scope={context.scope} administration={context.administration} selfId={context.self!.principalId} />}
      {current === "services" && <ServiceAccountsPanel scope={context.scope} administration={context.administration} />}
      {current === "storage" && <StoragePanel scope={context.scope} administration={context.administration} />}
      {current === "status" && <ServerStatusPanel scope={context.scope} administration={context.administration} />}
    </Tabs></div>;
}

function RemoteSettings({ context, detail }: { context: RemoteTeam; detail: string }) {
  const workspace = useWorkspace();
  const path = `${base(workspace.root)}/remote-settings`;
  const current = settingsSections.includes(detail as typeof settingsSections[number]) ? detail as typeof settingsSections[number] : "general";
  const [dirty, setDirty] = useState(false);
  useNavigationGuard(dirty ? "Des modifications ne sont pas enregistrées. Quitter cette page et les perdre ?" : null);
  const directory = useDirectory({ scope: context.scope, administration: context.administration, policy: context.policy,
    instanceAdmin: context.who?.instance_administrator === true, self: context.self, administer: context.capabilities.flags?.managePolicy ?? null });
  const layers: LayerRef[] = useMemo(
    () => (context.bootstrap?.layers ?? []).map((item) => ({ layerId: item.record.layerId, name: item.record.name, headStepIds: item.headStepIds })),
    [context.bootstrap],
  );
  const { query } = useLocation();
  const panel: Record<string, ReactNode> = {
    lenses: <ProjectLensesPanel key={context.scope} scope={context.scope} transport={context.administration} onDirtyChange={setDirty} />,
    general: <Card className="settings-card"><h2>Projet sur le serveur</h2><p>{context.projectName}</p><p>{layers.map((layer) => layer.name).join(", ") || "Aucun Layer publié"}</p>
      <Button size="sm" onClick={() => navigate(`${base(workspace.root)}/team`)}>Voir l’équipe et les accès</Button></Card>,
    protections: <ProtectionsPanel scope={context.scope} directory={directory} administration={context.administration} layers={layers}
      projectName={context.projectName} canConfigure={context.capabilities.flags?.configureProtection ?? null}
      initialLayer={query.get("layer") ?? undefined} onDirtyChange={setDirty} />,
    automations: <AutomationsPanel scope={context.scope} directory={directory} administration={context.administration} layers={layers}
      onEdit={(layer) => navigate(`${path}/protections`, { query: { layer } })} onOpenRuns={() => navigate(`${base(workspace.root)}/reviews`)} />,
    webhooks: <WebhooksPanel scope={context.scope} administration={context.administration} projectName={context.projectName}
      canManage={context.capabilities.flags?.manageWebhooks ?? null} />,
    advanced: <AdvancedPanel scope={context.scope} spaceId={context.spaceId} directory={directory} policy={context.policy}
      canManagePolicy={context.capabilities.flags?.managePolicy ?? null} projectName={context.projectName}
      onChanged={() => void context.capabilities.reload()} onDirtyChange={setDirty}
      desktopNote={<p>Import et export Git et accès limité se gèrent depuis les outils locaux de Layrs Desktop.</p>} />,
  };
  return <div className="layrs-page"><PageHeader title="Réglages sur le serveur" description="Protections, automatisations, webhooks et règles d’accès du projet." />
    <Tabs label="Réglages sur le serveur" value={current} onChange={(next) => navigate(`${path}/${next}`)}
      items={[{ id: "general", label: "Général" }, { id: "protections", label: "Protections" }, { id: "automations", label: "Automatisations" },
        { id: "webhooks", label: "Webhooks" }, { id: "advanced", label: "Avancé" }, { id: "lenses", label: "Lenses" }]}>
      {context.capabilities.loading && !context.capabilities.value ? <Loading label="Lecture de vos droits…" /> : panel[current]}
    </Tabs></div>;
}

function LimitedTeam() {
  const workspace = useWorkspace();
  const projection = useProjection();
  const remote = useRemote();
  const { data, patch, ready, target } = projection;
  const reason = workspace.busy
    ? `Opération en cours : ${workspace.busy}`
    : !data || !ready
      ? "Actualisez l’accès limité avant de continuer."
      : data.latestCaptureImported === true
        ? "Créez une capture locale dans cette copie avant de proposer un candidat."
      : !patch
        ? "Capturez d’abord vos modifications autorisées."
        : !target?.layerId
          ? "Choisissez le Layer distant cible dans Synchronisation."
          : data.capabilities.publish !== true
            ? "La publication n’est pas disponible dans cette connexion."
            : data.pendingPublication?.intent === "advance"
              ? "Une publication attend son reçu : relancez-la d’abord."
              : undefined;
  return <div className="layrs-page"><PageHeader title="Équipe" description="Propositions et revues des fichiers autorisés." />
    <Card className="team__card team__candidate"><div><h2>Proposer votre capture</h2>
      <p>Envoyez la dernière capture locale comme candidat : le Layer reste à sa version actuelle jusqu’à l’intégration d’une revue approuvée.</p></div>
      <Button icon={<Send aria-hidden size={16} />} disabledReason={reason} onClick={() => void projection.publish(true)}>
        {data?.pendingPublication?.intent === "candidate" ? "Relancer l’envoi pour revue" : "Envoyer la capture pour revue"}
      </Button></Card>
    {data && <Card className="team__card"><NativeScopedReviews key={`${workspace.root}:${data.identity.principalId}:${remote.epoch}`}
      root={workspace.root} origin={data.identity.origin} ca={remote.authority}
      disabled={Boolean(workspace.busy) || !ready} candidateId={projection.candidateId} /></Card>}
  </div>;
}
