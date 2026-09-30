import { useMemo, type ReactNode } from "react";
import { Bot, Gauge, HardDrive, Shield, Users } from "lucide-react";
import { AccountsPanel, EmptyState, Link, PageHeader, ServerStatusPanel, ServiceAccountsPanel, StoragePanel } from "@layrs/ui";
import { administrationTransport } from "../app/transports";
import { useSession } from "../app/session";

type Section = "accounts" | "services" | "storage" | "status";
const sections: { id: Section; label: string; icon: ReactNode }[] = [
  { id: "accounts", label: "Comptes", icon: <Users aria-hidden size={16} /> },
  { id: "services", label: "Comptes de service", icon: <Bot aria-hidden size={16} /> },
  { id: "storage", label: "Stockage", icon: <HardDrive aria-hidden size={16} /> },
  { id: "status", label: "État du serveur", icon: <Gauge aria-hidden size={16} /> },
];

/**
 * P21 · Administration de l’instance. Shown only when `whoami` reports an
 * instance administrator; every endpoint rechecks it on the server.
 */
export function AdminPage({ section }: { section?: string }) {
  const { api, principal } = useSession();
  const instance = useMemo(() => administrationTransport(api, ""), [api]);
  const current: Section = sections.some((item) => item.id === section) ? (section as Section) : "accounts";
  const scope = `${location.origin}:${principal?.principalId ?? ""}:instance`;

  if (!principal?.instanceAdministrator)
    return (
      <div className="layrs-page layrs-page--narrow">
        <PageHeader title="Administration de l’instance" />
        <EmptyState icon={<Shield size={22} />} title="Réservé aux administrateurs de l’instance">
          Votre compte n’administre pas cette instance. Les équipes et accès d’un projet se gèrent depuis le projet lui-même.
        </EmptyState>
      </div>
    );

  return (
    <div className="layrs-page">
      <PageHeader title="Administration de l’instance" description="Comptes, comptes de service, stockage et état du serveur. Ces réglages concernent toute l’instance." />
      <div className="team-settings">
        <nav className="team-settings__nav" aria-label="Sections de l’administration">
          <ul>
            {sections.map((item) => (
              <li key={item.id}>
                <Link to={`/admin/${item.id}`} aria-current={item.id === current ? "page" : undefined}>
                  {item.icon}
                  {item.label}
                </Link>
              </li>
            ))}
          </ul>
        </nav>
        <div>
          {current === "accounts" && <AccountsPanel scope={scope} administration={instance} selfId={principal.principalId} />}
          {current === "services" && <ServiceAccountsPanel scope={scope} administration={instance} />}
          {current === "storage" && <StoragePanel scope={scope} administration={instance} />}
          {current === "status" && <ServerStatusPanel scope={scope} administration={instance} />}
        </div>
      </div>
    </div>
  );
}
