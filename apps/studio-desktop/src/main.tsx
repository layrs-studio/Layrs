import React from "react";
import { createRoot } from "react-dom/client";
import "@layrs/ui/styles.css";
import "./desktop.css";
import { NativeWorkbench } from "./NativeWorkbench";

createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <NativeWorkbench />
  </React.StrictMode>
);
