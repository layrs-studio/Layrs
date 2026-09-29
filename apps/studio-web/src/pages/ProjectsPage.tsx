import { useState, type FormEvent } from "react";
import { FolderTree } from "lucide-react";
import { Button, Card, EmptyState, PageHeader, TextField, navigate, segment } from "@layrs/ui";
import { knownProjects } from "../app/project";

export function openProject(spaceId: string) {
  navigate(`/projects/${segment(spaceId)}/files`);
}

/**
 * P03. Until the authorized catalog is connected, projects are opened by
 * their reference; those opened in this session are listed.
 */
export function ProjectsPage() {
  const [reference, setReference] = useState("");
  const projects = knownProjects();

  function submit(event: FormEvent) {
    event.preventDefault();
    if (reference.trim()) openProject(reference.trim());
  }

  return (
    <div className="layrs-page">
      <PageHeader title="Vos projets" description="Consultez les projets de votre équipe et collaborez sur leurs revues." />
      {projects.length ? (
        <Card>
          <ul className="project-list">
            {projects.map((project) => (
              <li key={project.spaceId} className="project-list__item">
                <button type="button" className="project-list__open" onClick={() => openProject(project.spaceId)}>
                  <span className="project-list__mark" aria-hidden>
                    {project.name.replace(/^space:/, "").slice(0, 1).toUpperCase()}
                  </span>
                  <span className="project-list__text">
                    <span className="project-list__name">{project.name}</span>
                    <span className="project-list__path">Ouvert pendant cette session</span>
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </Card>
      ) : (
        <EmptyState icon={<FolderTree size={22} />} title="Ouvrez un projet de votre équipe">
          La liste des projets accessibles à votre compte arrive. En attendant, ouvrez un projet avec la référence que vous a
          transmise son responsable.
        </EmptyState>
      )}
      <Card>
        <form className="settings-card open-by-reference" onSubmit={submit}>
          <TextField
            label="Référence du projet"
            value={reference}
            placeholder="space:…"
            hint="Visible dans les réglages du projet sur Desktop ou fournie par votre équipe."
            onChange={(event) => setReference(event.target.value)}
          />
          <div>
            <Button type="submit" disabledReason={!reference.trim() ? "Saisissez une référence." : undefined}>
              Ouvrir le projet
            </Button>
          </div>
        </form>
      </Card>
    </div>
  );
}
