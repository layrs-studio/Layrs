import React from "react";
import { createRoot } from "react-dom/client";
import { NotificationProvider } from "@layrs/ui";
import "@layrs/ui/styles.css";
import "@layrs/lenses/styles.css";
import "./studio.css";
import { StudioApp } from "./StudioApp";

createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <NotificationProvider>
      <StudioApp />
    </NotificationProvider>
  </React.StrictMode>
);
