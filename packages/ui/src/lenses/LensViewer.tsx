/// <reference path="./raw-import.d.ts" />
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import localHost from "../../../../deploy/lens-runtime.html?raw";
import { useLensRuntime, selectProjectLens } from "./runtime";
import { pinKey, type LensView, type LensViewInput } from "./types";
import { validateView, verifyViewer } from "./verify";

/** Only the trusted host gets a MessagePort. Extension code runs in its worker. */
export function LensViewer({ input, fallback }: { input: LensViewInput; fallback?: ReactNode }) {
  const runtime = useLensRuntime();
  const item = runtime?.configuration ? selectProjectLens(runtime.configuration, input) : undefined;
  const key = `${runtime?.scope}|${runtime?.configuration?.revision}|${item ? pinKey(item.pin) : ""}|${JSON.stringify(input)}`;
  return <View key={key} input={input} fallback={fallback} />;
}
function View({ input, fallback }: { input: LensViewInput; fallback?: ReactNode }) {
  const runtime = useLensRuntime();
  const item = runtime?.configuration ? selectProjectLens(runtime.configuration, input) : undefined;
  const [loaded, setLoaded] = useState<Awaited<ReturnType<typeof verifyViewer>> | null>(null);
  const [view, setView] = useState<LensView | null>(null);
  const [error, setError] = useState("");
  const frame = useRef<HTMLIFrameElement>(null);
  const [ready, setReady] = useState(false);
  const serialized = useMemo(() => JSON.stringify(input), [input]);
  useEffect(() => {
    if (!runtime || !item) return;
    let alive = true;
    runtime.load(item).then(value => { if (alive) setLoaded(value); }, () => { if (alive) setError("Le composant client n’a pas pu être vérifié."); });
    return () => { alive = false; };
  }, [runtime?.load, item]);
  useEffect(() => {
    if (!ready || !loaded || !frame.current?.contentWindow) return;
    const channel = new MessageChannel();
    let finished = false;
    const deadline = window.setTimeout(() => { finished = true; setError("La lens n’a pas répondu à temps."); channel.port1.close(); }, 7000);
    channel.port1.onmessage = ({ data }) => {
      if (finished) return;
      if (data?.ready === true) {
        channel.port1.postMessage({ ...loaded, input: JSON.parse(serialized) });
        return;
      }
      finished = true;
      clearTimeout(deadline);
      try {
        if (typeof data?.value !== "string" || data.value.length > 1024 * 1024) throw Error();
        setView(validateView(JSON.parse(data.value)));
      } catch { setError("Cette lens ne peut pas afficher ce contenu."); }
      channel.port1.close();
    };
    frame.current.contentWindow.postMessage("layrs.lens.worker.v1", "*", [channel.port2]);
    return () => { finished = true; clearTimeout(deadline); channel.port1.close(); };
  }, [loaded, ready, serialized]);
  if (!runtime || !item) return <>{fallback}</>;
  return <section aria-label={`Lens ${item.manifest.name}`}>
    <p className="team-muted">{item.manifest.name} · {item.pin.version} · {view ? "Chargée" : error ? "Indisponible" : "Chargement…"}{runtime.offline ? " · Cache hors ligne" : ""}</p>
    {!error && loaded && <iframe ref={frame} title="Exécution isolée de la lens" hidden sandbox="allow-scripts" referrerPolicy="no-referrer"
      src={runtime.hostUrl} srcDoc={runtime.hostUrl ? undefined : localHost} onLoad={() => setReady(true)} />}
    {error && <p role="status">{error} Affichage de secours.</p>}
    {view ? <div>{view.title && <h3>{view.title}</h3>}{view.blocks.map((block, index) => block.kind === "heading" ? <h4 key={index}>{block.text}</h4> : block.kind === "code" ? <pre key={index} style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>{block.text}</pre> : <p key={index}>{block.text}</p>)}</div> : fallback}
  </section>;
}
