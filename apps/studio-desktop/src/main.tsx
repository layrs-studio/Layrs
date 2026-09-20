import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { NotificationProvider } from "@layrs/ui";
import "@layrs/ui/styles.css";
import "@layrs/lenses/styles.css";
import "./desktop.css";
import { DesktopApp } from "./DesktopApp";
import { NativeWorkbench } from "./NativeWorkbench";

function DesktopRoot() {
  const [project, setProject] = useState(window.location.hash !== "#local");
  useEffect(() => { const changed = () => setProject(window.location.hash !== "#local"); window.addEventListener("hashchange", changed); return () => window.removeEventListener("hashchange", changed); }, []);
  return project ? <NativeWorkbench /> : <><DesktopApp /><a className="native-launcher" href="#project">Ouvrir un projet local</a></>;
}

createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <NotificationProvider>
      <DesktopRoot />
    </NotificationProvider>
  </React.StrictMode>
);
