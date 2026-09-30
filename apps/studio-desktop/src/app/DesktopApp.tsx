import { useEffect, type ReactNode } from "react";
import { AppearanceSettings, ToastProvider, matchRoute, navigate, segment, useLocation } from "@layrs/ui";
import { NewProjectPage } from "../pages/NewProjectPage";
import { ProjectsPage } from "../pages/ProjectsPage";
import { Frame } from "./Frame";
import { ProjectRoute } from "./ProjectFrame";
import { readRecents } from "./recents";

export function DesktopApp() {
  return (
    <ToastProvider>
      <Router />
    </ToastProvider>
  );
}

function Router() {
  const { path } = useLocation();

  // Desktop opens the most recent local project; without one, the project list.
  useEffect(() => {
    if (path !== "/") return;
    const last = readRecents()[0];
    navigate(last ? `/projects/${segment(last.root)}/changes` : "/projects", { replace: true });
  }, [path]);

  const project = matchRoute("/projects/:project/*", path);
  if (project && project.project !== "new") return <ProjectRoute root={project.project} rest={project["*"]} />;

  let page: ReactNode;
  let title = "Projets";
  if (matchRoute("/projects/new", path)) {
    page = <NewProjectPage />;
    title = "Nouveau projet";
  } else if (matchRoute("/settings/appearance", path)) {
    page = <AppearanceSettings surface="desktop" />;
    title = "Apparence";
  } else page = <ProjectsPage />;
  return <Frame crumbs={[{ label: "Projets", to: "/projects" }, ...(title === "Projets" ? [] : [{ label: title }])]}>{page}</Frame>;
}
