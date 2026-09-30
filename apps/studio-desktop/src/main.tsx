import React from "react";
import { createRoot } from "react-dom/client";
import "@layrs/ui/layrs.css";
import "./desktop.css";
import "./work.css";
import { DesktopApp } from "./app/DesktopApp";

createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <DesktopApp />
  </React.StrictMode>
);
