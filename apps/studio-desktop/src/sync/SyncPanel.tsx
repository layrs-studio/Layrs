import { useEffect, useId, useRef, useState } from "react";
import { ArrowRight, RefreshCw, X } from "lucide-react";
import { Link, Spinner, segment } from "@layrs/ui";
import { useRemote } from "../app/remote";
import { useWorkspace } from "../app/workspace";
import { ActivityList, NoticeView, PendingState, ServerSummary, SyncActions, TransferView, sessionSummary } from "./parts";

/**
 * P11: permanent top-bar access to server state, Publish/Receive and the
 * running transfer. Closing the panel never interrupts the transfer.
 */
export function SyncButton() {
  const remote = useRemote();
  const workspace = useWorkspace();
  const [open, setOpen] = useState(false);
  const [unseen, setUnseen] = useState(false);
  const panelId = useId();
  const buttonRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const summary = sessionSummary(remote.session);
  const failed = remote.notice?.tone === "danger" || remote.notice?.tone === "warning";

  useEffect(() => {
    if (remote.notice && !open) setUnseen(true);
    // Only a new notice raises the marker.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [remote.notice]);

  useEffect(() => {
    if (!open) return;
    setUnseen(false);
    function onPointerDown(event: PointerEvent) {
      const target = event.target as Node;
      if (!panelRef.current?.contains(target) && !buttonRef.current?.contains(target) && !(target instanceof Element && target.closest("dialog"))) setOpen(false);
    }
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !document.querySelector("dialog[open]")) {
        setOpen(false);
        buttonRef.current?.focus();
      }
    }
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  useEffect(() => {
    if (open) panelRef.current?.focus();
  }, [open]);

  const state = remote.transfer ? remote.transfer.label : summary.label;
  return (
    <div className="sync-anchor">
      <button
        ref={buttonRef}
        type="button"
        className="sync-trigger"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        onClick={() => setOpen((value) => !value)}
      >
        {remote.transfer ? <Spinner size={14} /> : <RefreshCw aria-hidden size={16} />}
        <span className="sync-trigger__text">
          <span className="sync-trigger__label">Synchronisation</span>
          <span className={`sync-trigger__state sync-trigger__state--${remote.transfer ? "accent" : summary.tone}`}>{state}</span>
        </span>
        {unseen && failed && <span className="sync-trigger__alert" aria-label="Résultat à consulter" />}
      </button>
      {open && (
        <div ref={panelRef} id={panelId} className="sync-panel" role="dialog" aria-label="Synchronisation" tabIndex={-1}>
          <header className="sync-panel__head">
            <h2>Synchronisation</h2>
            <button type="button" className="sync-panel__close" aria-label="Fermer la synchronisation" onClick={() => setOpen(false)}>
              <X aria-hidden size={16} />
            </button>
          </header>
          <ServerSummary compact />
          <p className="sync-panel__hint">
            Publier envoie vos captures et attend leur acceptation par le serveur. Recevoir récupère l’état autorisé de l’équipe.
          </p>
          <PendingState />
          <SyncActions />
          {remote.transfer && <TransferView transfer={remote.transfer} />}
          <NoticeView notice={remote.notice} onClose={remote.clearNotice} />
          <section aria-label="Activité récente" className="sync-panel__activity">
            <h3>Activité récente</h3>
            <ActivityList entries={remote.activity} limit={3} />
          </section>
          <Link className="sync-panel__more" to={`/projects/${segment(workspace.root)}/sync`} onClick={() => setOpen(false)}>
            Détails, reprise et options avancées <ArrowRight aria-hidden size={14} />
          </Link>
        </div>
      )}
    </div>
  );
}
