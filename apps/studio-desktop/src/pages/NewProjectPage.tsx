import { useState } from "react";
import { CloudDownload, FolderPlus } from "lucide-react";
import { Banner, Button, Card, PageHeader, SegmentedControl, TextField, navigate, useLocation } from "@layrs/ui";
import { nativeWorkspace, selectFolder } from "../tauri";
import { NativeRemotePanel } from "../NativeRemotePanel";
import { rememberProject } from "../app/recents";
import { openProject } from "./ProjectsPage";

type Mode = "initialize" | "clone";

/** P04: start working, adapted to the chosen intention. */
export function NewProjectPage() {
  const { query } = useLocation();
  const [mode, setMode] = useState<Mode>(query.get("mode") === "clone" ? "clone" : "initialize");
  const [root, setRoot] = useState(query.get("root") ?? "");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function browse() {
    setError("");
    try {
      const selected = await selectFolder(root);
      if (selected) setRoot(selected);
    } catch (cause) {
      setError(String(cause));
    }
  }

  async function initialize() {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await nativeWorkspace(root, { action: "initialize", name: name.trim() });
      rememberProject({ root, name: name.trim(), kind: "complete" });
      openProject(root);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  const folderField = (
    <div className="new-project__folder">
      <TextField
        label="Dossier"
        value={root}
        placeholder="Chemin du dossier"
        hint={mode === "clone" ? "Choisissez un dossier vide : rien n’y est écrit avant la confirmation." : "Les fichiers déjà présents sont conservés."}
        onChange={(event) => setRoot(event.target.value)}
      />
      <Button onClick={() => void browse()}>Parcourir…</Button>
    </div>
  );

  return (
    <div className="layrs-page layrs-page--narrow">
      <PageHeader title="Nouveau projet" description="Que voulez-vous faire ?" />
      <SegmentedControl<Mode>
        label="Type de projet"
        value={mode}
        onChange={(next) => {
          setMode(next);
          setError("");
        }}
        options={[
          { value: "initialize", label: "Commencer dans un dossier", icon: <FolderPlus aria-hidden size={16} /> },
          { value: "clone", label: "Récupérer un projet de l’équipe", icon: <CloudDownload aria-hidden size={16} /> },
        ]}
      />
      {error && (
        <Banner tone="danger" title="Le projet n’a pas pu être préparé">
          {error}
        </Banner>
      )}
      {mode === "initialize" ? (
        <Card className="new-project__card">
          {folderField}
          <TextField label="Nom du nouveau projet" value={name} onChange={(event) => setName(event.target.value)} />
          <p className="new-project__note">Layrs suit ce dossier et capture son état actuel. Aucun fichier n’est envoyé à un serveur.</p>
          <div className="new-project__actions">
            <Button onClick={() => navigate("/projects")}>Annuler</Button>
            <Button
              variant="primary"
              busy={busy}
              busyLabel="Initialisation…"
              disabledReason={!root ? "Choisissez un dossier." : !name.trim() ? "Donnez un nom au projet." : undefined}
              onClick={() => void initialize()}
            >
              Initialiser
            </Button>
          </div>
        </Card>
      ) : (
        <Card className="new-project__card">
          {folderField}
          <div className="layrs-legacy">
            <NativeRemotePanel
              key={root}
              root={root}
              opened={false}
              busy={busy}
              onBusy={setBusy}
              onChanged={(next) => {
                rememberProject({ root: next, name: "", kind: "complete" });
                openProject(next);
              }}
            />
          </div>
        </Card>
      )}
    </div>
  );
}
