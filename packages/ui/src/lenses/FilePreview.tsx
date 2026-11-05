import { useEffect, useState } from "react";
import { LensViewer } from "./LensViewer";
import type { LensTransport } from "./types";
export function FilePreview({ path, stepId, policySetId, authorizationEpoch, transport }: {
  path: string; stepId: string; policySetId: string; authorizationEpoch: number; transport: LensTransport;
}) {
  const [value, setValue] = useState<{ text: string | null; omitted: boolean } | null>(null);
  const [error, setError] = useState("");
  useEffect(() => { let alive = true;
    setValue(null); setError("");
    transport<{ path: string; stepId: string; text: string | null; omitted: boolean }>({ action: "filePreview", request: { path, stepId, expectedPolicySetId: policySetId, expectedAuthorizationEpoch: authorizationEpoch } }).then(next => {
      if (!alive) return;
      if (next.path !== path || next.stepId !== stepId) { setError("Le contenu reçu ne correspond pas au fichier sélectionné."); return; }
      setValue(next);
    }, () => { if (alive) setError("Aperçu indisponible. Actualisez le projet pour vérifier vos droits."); });
    return () => { alive = false; };
  }, [transport, path, stepId, policySetId, authorizationEpoch]);
  if (error) return <p role="status">{error}</p>;
  if (!value) return <p role="status">Chargement du contenu…</p>;
  if (value.omitted || value.text === null) return <p>Affichage texte indisponible pour ce format ou ce fichier de plus de 1 Mio.</p>;
  return <LensViewer input={{ kind: "preview", path, text: value.text }} fallback={<pre style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>{value.text}</pre>} />;
}
