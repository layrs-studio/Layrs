import React from "react";
import { createRoot } from "react-dom/client";
import "@layrs/ui/styles.css";
import "@layrs/lenses/styles.css";
import "./desktop.css";
import { DesktopApp } from "./DesktopApp";

createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <DesktopApp />
  </React.StrictMode>
);
