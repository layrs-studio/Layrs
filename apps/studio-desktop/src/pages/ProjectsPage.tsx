import { useState } from "react";
import { FolderOpen, FolderTree, MoreHorizontal, Plus, Star } from "lucide-react";
import { Badge, Button, Card, EmptyState, Menu, PageHeader, formatRelativeTime, navigate, segment, useToast } from "@layrs/ui";
import { selectFolder } from "../tauri";
import { forgetProject, readRecents, toggleFavorite, type RecentProject } from "../app/recents";

export function openProject(root: string) {
  navigate(`/projects/${segment(root)}/changes`);
}

export function ProjectsPage() {
  const toast = useToast();
  const [recents, setRecents] = useState(readRecents);
  const sorted = [...recents].sort((a, b) => Number(Boolean(b.favorite)) - Number(Boolean(a.favorite)) || b.openedAt.localeCompare(a.openedAt));

  async function browse() {
    try {
      const root = await selectFolder();
      if (root) openProject(root);
    } catch (error) {
      toast.show({ tone: "danger", title: "Impossible d’ouvrir le sélecteur de dossier", description: String(error) });
    }
  }

  return (
    <div className="layrs-page">
      <PageHeader
        title="Vos projets"
        description="Reprenez un projet de cet ordinateur ou commencez-en un nouveau."
        aside={
          <>
            <Button icon={<FolderOpen aria-hidden size={16} />} onClick={() => void browse()}>
              Ouvrir un dossier
            </Button>
            <Button variant="primary" icon={<Plus aria-hidden size={16} />} onClick={() => navigate("/projects/new")}>
              Nouveau projet
            </Button>
          </>
        }
      />
      <section aria-labelledby="local-projects" className="projects-section">
        <h2 id="local-projects">Sur cet ordinateur</h2>
        {sorted.length ? (
          <Card>
            <ul className="project-list">
              {sorted.map((project) => (
                <ProjectRow
                  key={project.root}
                  project={project}
                  onChange={() => setRecents(readRecents())}
                />
              ))}
            </ul>
          </Card>
        ) : (
          <EmptyState
            icon={<FolderTree size={22} />}
            title="Aucun projet ouvert sur cet ordinateur"
            actions={
              <>
                <Button icon={<FolderOpen aria-hidden size={16} />} onClick={() => void browse()}>
                  Ouvrir un dossier
                </Button>
                <Button variant="primary" onClick={() => navigate("/projects/new")}>
                  Nouveau projet
                </Button>
              </>
            }
          >
            Ouvrez un dossier déjà suivi par Layrs, commencez un projet dans un dossier, ou récupérez un projet de votre équipe.
          </EmptyState>
        )}
      </section>
      <section aria-labelledby="team-projects" className="projects-section">
        <h2 id="team-projects">Dans l’équipe</h2>
        <Card className="projects-team">
          <p>
            Récupérez un projet partagé sur le serveur de votre équipe dans un dossier vide de cet ordinateur.
          </p>
          <Button onClick={() => navigate("/projects/new", { query: { mode: "clone" } })}>Récupérer un projet de l’équipe</Button>
        </Card>
      </section>
    </div>
  );
}

function ProjectRow({ project, onChange }: { project: RecentProject; onChange: () => void }) {
  return (
    <li className="project-list__item">
      <button type="button" className="project-list__open" onClick={() => openProject(project.root)}>
        <span className="project-list__mark" aria-hidden>
          {project.name.slice(0, 1).toUpperCase() || "?"}
        </span>
        <span className="project-list__text">
          <span className="project-list__name">
            {project.name}
            {project.favorite && <Star aria-label="Favori" size={14} className="project-list__star" />}
          </span>
          <span className="project-list__path">{project.root}</span>
        </span>
        {project.kind === "projection" && <Badge tone="warning">Accès limité</Badge>}
        {project.openedAt > new Date(0).toISOString() && (
          <span className="project-list__time">Ouvert {formatRelativeTime(project.openedAt)}</span>
        )}
      </button>
      <Menu
        label={`Actions pour ${project.name}`}
        trigger={<MoreHorizontal aria-hidden size={18} />}
        items={[
          {
            id: "favorite",
            label: project.favorite ? "Retirer des favoris" : "Ajouter aux favoris",
            icon: <Star aria-hidden size={16} />,
            onSelect: () => {
              toggleFavorite(project.root);
              onChange();
            },
          },
          {
            id: "forget",
            label: "Retirer des récents",
            danger: true,
            onSelect: () => {
              forgetProject(project.root);
              onChange();
            },
          },
        ]}
      />
    </li>
  );
}
