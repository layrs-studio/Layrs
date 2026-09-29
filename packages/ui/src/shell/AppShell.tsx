import { useEffect, useState, type ReactNode } from "react";
import { ChevronRight, Menu as MenuIcon, PanelLeftClose, PanelLeftOpen, Search, X } from "lucide-react";
import { Kbd } from "../components/Feedback";
import { Brand } from "./Brand";
import { Link, useLocation } from "./router";

const SHELL_STORAGE_KEY = "layrs.shell.v1";

function readCollapsed() {
  try {
    return globalThis.localStorage?.getItem(SHELL_STORAGE_KEY) === "rail";
  } catch {
    return false;
  }
}

/**
 * Stable application frame: sidebar (collapsible to a rail, drawer on narrow
 * windows), top bar and the main region. It never unmounts during loading.
 */
export function AppShell({
  sidebar,
  sidebarFooter,
  topbar,
  topbarActions,
  children,
}: {
  /** Context switcher, projects and project navigation. */
  sidebar: ReactNode;
  /** Settings, theme and server status, pinned to the bottom. */
  sidebarFooter?: ReactNode;
  /** Breadcrumbs and Layer selector. */
  topbar?: ReactNode;
  /** Search, activity, inbox, account. */
  topbarActions?: ReactNode;
  children: ReactNode;
}) {
  const [collapsed, setCollapsed] = useState(readCollapsed);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const { path } = useLocation();

  // Navigating closes the narrow-window drawer.
  useEffect(() => setDrawerOpen(false), [path]);

  function toggleRail() {
    setCollapsed((value) => {
      try {
        globalThis.localStorage?.setItem(SHELL_STORAGE_KEY, value ? "full" : "rail");
      } catch {
        // Session-only preference.
      }
      return !value;
    });
  }

  return (
    <div className={["layrs-shell", collapsed && "layrs-shell--rail", drawerOpen && "layrs-shell--drawer-open"].filter(Boolean).join(" ")}>
      <a className="layrs-skip-link" href="#layrs-main" onClick={(event) => {
        event.preventDefault();
        document.getElementById("layrs-main")?.focus();
      }}>
        Aller au contenu
      </a>
      <aside className="layrs-sidebar" aria-label="Navigation principale">
        <div className="layrs-sidebar__brand">
          <Brand compact={collapsed} />
          <button
            type="button"
            className="layrs-sidebar__drawer-close"
            aria-label="Fermer la navigation"
            onClick={() => setDrawerOpen(false)}
          >
            <X aria-hidden size={18} />
          </button>
        </div>
        <div className="layrs-sidebar__content">{sidebar}</div>
        <div className="layrs-sidebar__footer">
          {sidebarFooter}
          <button
            type="button"
            className="layrs-sidebar__collapse"
            aria-pressed={collapsed}
            onClick={toggleRail}
            title={collapsed ? "Déplier la navigation" : "Réduire la navigation"}
          >
            {collapsed ? <PanelLeftOpen aria-hidden size={18} /> : <PanelLeftClose aria-hidden size={18} />}
            <span className="layrs-nav__label">{collapsed ? "Déplier" : "Réduire la navigation"}</span>
          </button>
        </div>
      </aside>
      <div className="layrs-shell__backdrop" aria-hidden onClick={() => setDrawerOpen(false)} />
      <div className="layrs-shell__body">
        <header className="layrs-topbar">
          <button
            type="button"
            className="layrs-topbar__menu"
            aria-label="Ouvrir la navigation"
            aria-expanded={drawerOpen}
            onClick={() => setDrawerOpen(true)}
          >
            <MenuIcon aria-hidden size={20} />
          </button>
          <div className="layrs-topbar__context">{topbar}</div>
          <div className="layrs-topbar__actions">{topbarActions}</div>
        </header>
        <main id="layrs-main" className="layrs-main" tabIndex={-1}>
          {children}
        </main>
      </div>
    </div>
  );
}

export function NavSection({ title, actions, children }: { title?: string; actions?: ReactNode; children: ReactNode }) {
  return (
    <section className="layrs-nav-section" aria-label={title}>
      {title && (
        <div className="layrs-nav-section__head">
          <p className="layrs-nav-section__title">{title}</p>
          {actions && <div className="layrs-nav-section__actions">{actions}</div>}
        </div>
      )}
      <ul className="layrs-nav">{children}</ul>
    </section>
  );
}

/** Navigation entry; active when the current path starts with `to` (or equals it with `exact`). */
export function NavItem({
  to,
  icon,
  label,
  badge,
  exact,
  dot,
}: {
  to: string;
  icon?: ReactNode;
  label: string;
  /** Real count only, rendered as text for assistive technology. */
  badge?: { count: number; label: string };
  exact?: boolean;
  /** Small colour dot, e.g. for projects. */
  dot?: "accent" | "muted";
}) {
  const { path } = useLocation();
  const active = exact ? path === to : path === to || path.startsWith(`${to}/`);
  return (
    <li>
      <Link to={to} className="layrs-nav__item" aria-current={active ? "page" : undefined} title={label}>
        {dot ? <span className={`layrs-nav__dot layrs-nav__dot--${dot}`} aria-hidden /> : icon}
        <span className="layrs-nav__label">{label}</span>
        {badge && badge.count > 0 && (
          <span className="layrs-nav__badge" aria-label={badge.label}>
            {badge.count}
          </span>
        )}
      </Link>
    </li>
  );
}

export type Crumb = { label: ReactNode; to?: string };

export function Breadcrumbs({ items }: { items: Crumb[] }) {
  return (
    <nav aria-label="Fil d’Ariane" className="layrs-breadcrumbs">
      <ol>
        {items.map((item, index) => {
          const last = index === items.length - 1;
          return (
            <li key={index}>
              {item.to && !last ? <Link to={item.to}>{item.label}</Link> : <span aria-current={last ? "page" : undefined}>{item.label}</span>}
              {!last && <ChevronRight aria-hidden size={14} className="layrs-breadcrumbs__separator" />}
            </li>
          );
        })}
      </ol>
    </nav>
  );
}

/** Top-bar entry to the command palette. */
export function SearchTrigger({ onOpen, label = "Rechercher…" }: { onOpen: () => void; label?: string }) {
  const mac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
  return (
    <button type="button" className="layrs-search-trigger" onClick={onOpen} aria-keyshortcuts={mac ? "Meta+K" : "Control+K"}>
      <Search aria-hidden size={16} />
      <span className="layrs-search-trigger__label">{label}</span>
      <span className="layrs-search-trigger__keys" aria-hidden>
        <Kbd>{mac ? "⌘" : "Ctrl"}</Kbd>
        <Kbd>K</Kbd>
      </span>
    </button>
  );
}

/** Page heading with an optional status/action area on the right. */
export function PageHeader({ title, description, aside }: { title: ReactNode; description?: ReactNode; aside?: ReactNode }) {
  return (
    <div className="layrs-page-header">
      <div>
        <h1>{title}</h1>
        {description && <p className="layrs-page-header__description">{description}</p>}
      </div>
      {aside && <div className="layrs-page-header__aside">{aside}</div>}
    </div>
  );
}
