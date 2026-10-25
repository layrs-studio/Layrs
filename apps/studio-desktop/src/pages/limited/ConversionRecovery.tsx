import { useEffect, useRef, useState } from "react";
import { Banner, Button, Card, TextField } from "@layrs/ui";
import { useWorkspace } from "../../app/workspace";
import { abandonConversionRequest, abandonedConversion } from "../../app/offline";
import { nativeProjection } from "../../tauri";

type Execute = <T>(label: string, work: () => Promise<T>) => Promise<T | null | undefined>;

/** Also available when no source checkout can open, without a projection context. */
export function ConversionRecovery({ execute }: { execute?: Execute }) {
  const [family, setFamily] = useState(""), [destination, setDestination] = useState("");
  const { busy } = useWorkspace();
  return <section className="settings-section" aria-labelledby="conversion-recovery">
    <h2 id="conversion-recovery">Conversion interrompue</h2>
    <Card className="settings-card">
      <p>Si la source est indisponible, vous pouvez abandonner une conversion non terminée en indiquant son stockage et sa destination. Cette action ne nécessite pas d’ouvrir la source.</p>
      <TextField label="Stockage de la conversion interrompue" value={family} disabled={Boolean(busy)} onChange={e => setFamily(e.target.value)} />
      <TextField label="Destination de la conversion interrompue" value={destination} disabled={Boolean(busy)} onChange={e => setDestination(e.target.value)} />
      <AbandonConversionAction family={family} destination={destination} execute={execute} />
    </Card>
  </section>;
}

export function AbandonConversionAction({ family, destination, execute, onAbandoned }: { family: string; destination: string; execute?: Execute; onAbandoned?: () => void }) {
  const workspace = useWorkspace();
  const [receipt, setReceipt] = useState(""), [error, setError] = useState("");
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => { setReceipt(""); setError(""); }, [family, destination]);
  let request: ReturnType<typeof abandonConversionRequest> | undefined, invalid: string | undefined;
  try { request = abandonConversionRequest(family, destination); }
  catch (cause) { invalid = cause instanceof Error ? cause.message : String(cause); }
  const blocked = workspace.busy ? "Opération en cours." : invalid;
  async function abandon() {
    if (blocked || !request) return;
    setReceipt(""); setError("");
    const result = await (execute ?? workspace.task)("Abandon de la conversion…", async () => {
      try {
        return abandonedConversion(await nativeProjection(workspace.root, { action: "offline", request }), request.destination);
      } catch (cause) {
        if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause));
        throw cause;
      }
    });
    if (result && mounted.current) { setReceipt(result); onAbandoned?.(); }
  }
  return <div>
    <p>Seule une destination non prête peut être abandonnée. Sa source, son dossier et ses fichiers physiques restent conservés. Une copie déjà acceptée ou possédant de nouvelles captures sera refusée.</p>
    <Button disabledReason={blocked ?? (receipt ? "Cet abandon est déjà confirmé." : undefined)} onClick={() => void abandon()}>Abandonner cette conversion en conservant les fichiers</Button>
    {receipt && <p role="status">Conversion abandonnée : <code>{receipt}</code>. Fichiers conservés sur disque ; aucune ouverture ni suppression automatique.</p>}
    {error && <Banner tone="danger" title="L’abandon n’a pas abouti.">{error}</Banner>}
  </div>;
}
