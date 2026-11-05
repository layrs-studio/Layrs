import React from "react";
import { createRoot } from "react-dom/client";
import "@layrs/ui/layrs.css";
import "./web.css";
import { WebApp } from "./app/WebApp";

createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <WebApp />
  </React.StrictMode>
);
