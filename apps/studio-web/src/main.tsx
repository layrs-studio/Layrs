import React from "react";
import { createRoot } from "react-dom/client";
import "@layrs/ui/styles.css";
import { NativeStudio } from "./NativeStudio";

createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <NativeStudio />
  </React.StrictMode>
);
