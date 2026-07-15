import { ConfirmModal } from "@layrs/ui";
import type { ReactNode } from "react";
import type { CommandKey } from "./desktopTypes";
import type { LocalLayerSummary, LocalSpaceSummary } from "./tauri";

interface DesktopConfirmationsProps {
  activeParentLayer: LocalLayerSummary | null;
  busyAction: string | null;
  clearStepsTarget: LocalLayerSummary | null;
  commandErrors: Partial<Record<CommandKey, string>>;
  confirmWeaveParent: boolean;
  deleteLayerTarget: LocalLayerSummary | null;
  disconnectLayerTarget: LocalLayerSummary | null;
  forgetTarget: LocalSpaceSummary | null;
  selectedLayer: LocalLayerSummary | null;
  onCancelClearSteps: () => void;
  onCancelDeleteLayer: () => void;
  onCancelDisconnectLayer: () => void;
  onCancelForget: () => void;
  onCancelWeaveParent: () => void;
  onConfirmClearSteps: (layerId: string) => void;
  onConfirmDeleteLayer: (layerId: string) => void;
  onConfirmDisconnectLayer: (layerId: string) => void;
  onConfirmForget: (localSpaceId: string) => void;
  onConfirmWeaveParent: () => void;
}

export function DesktopConfirmations({
  activeParentLayer,
  busyAction,
  clearStepsTarget,
  commandErrors,
  confirmWeaveParent,
  deleteLayerTarget,
  disconnectLayerTarget,
  forgetTarget,
  selectedLayer,
  onCancelClearSteps,
  onCancelDeleteLayer,
  onCancelDisconnectLayer,
  onCancelForget,
  onCancelWeaveParent,
  onConfirmClearSteps,
  onConfirmDeleteLayer,
  onConfirmDisconnectLayer,
  onConfirmForget,
  onConfirmWeaveParent
}: DesktopConfirmationsProps) {
  const locked = Boolean(busyAction);
  return (
    <>
      <ConfirmModal
        confirmLabel="Forget local"
        danger
        description={
          <ConfirmationCopy error={commandErrors["forget-local"]}>
            <p>
              Layrs will keep the project files, archive local .layrs metadata, and disconnect this folder from Studio so
              it can be pulled again.
            </p>
          </ConfirmationCopy>
        }
        disabled={!forgetTarget || locked}
        onCancel={() => !locked && onCancelForget()}
        onConfirm={() => forgetTarget && onConfirmForget(forgetTarget.localSpaceId)}
        open={Boolean(forgetTarget)}
        title={`Forget ${forgetTarget?.name ?? "Local Space"}`}
      />
      <ConfirmModal
        confirmLabel="Delete Layer"
        danger
        description={
          <ConfirmationCopy error={commandErrors["delete-layer"]}>
            <p>Deleting a Layer removes its local Layer state. Keep this action away from receive and publish.</p>
          </ConfirmationCopy>
        }
        disabled={!deleteLayerTarget || locked}
        onCancel={() => !locked && onCancelDeleteLayer()}
        onConfirm={() => deleteLayerTarget && onConfirmDeleteLayer(deleteLayerTarget.layerId)}
        open={Boolean(deleteLayerTarget)}
        title={`Delete ${deleteLayerTarget?.displayName ?? "Layer"}`}
      />
      <ConfirmModal
        confirmLabel="Disconnect Layer"
        danger
        description={
          <ConfirmationCopy error={commandErrors["disconnect-layer"]}>
            <p>
              Future Steps from the parent Layer will no longer flow into this Layer automatically. Existing files and
              Steps stay in place.
            </p>
          </ConfirmationCopy>
        }
        disabled={!disconnectLayerTarget || locked}
        onCancel={() => !locked && onCancelDisconnectLayer()}
        onConfirm={() => disconnectLayerTarget && onConfirmDisconnectLayer(disconnectLayerTarget.layerId)}
        open={Boolean(disconnectLayerTarget)}
        title={`Disconnect ${disconnectLayerTarget?.displayName ?? "Layer"} from parent`}
      />
      <ConfirmModal
        confirmLabel="Clear Steps"
        danger
        description={
          <ConfirmationCopy error={commandErrors["clear-steps"]}>
            <p>
              Layrs will remove this Layer history from the active timeline and archive the Step metadata. The files in the
              folder and object store are kept.
            </p>
          </ConfirmationCopy>
        }
        disabled={!clearStepsTarget || locked}
        onCancel={() => !locked && onCancelClearSteps()}
        onConfirm={() => clearStepsTarget && onConfirmClearSteps(clearStepsTarget.layerId)}
        open={Boolean(clearStepsTarget)}
        title={`Clear Steps from ${clearStepsTarget?.displayName ?? "Layer"}`}
      />
      <ConfirmModal
        confirmLabel="Weave to parent"
        description={
          <ConfirmationCopy error={commandErrors["weave-parent"]}>
            <p>
              Layrs will move the current Layer Steps into parent Layer {activeParentLayer?.displayName ?? "parent"} using
              the durable Weave flow. If conflicts appear, you can resolve or abort from the Weaves page.
            </p>
          </ConfirmationCopy>
        }
        disabled={!selectedLayer || !activeParentLayer || locked}
        onCancel={() => !locked && onCancelWeaveParent()}
        onConfirm={onConfirmWeaveParent}
        open={confirmWeaveParent}
        title={`Weave ${selectedLayer?.displayName ?? "Layer"} to ${activeParentLayer?.displayName ?? "parent"}`}
      />
    </>
  );
}

function ConfirmationCopy({ children, error }: { children: ReactNode; error?: string }) {
  return (
    <>
      {children}
      {error ? <p className="desktop-alert desktop-alert--error" role="alert">{error}</p> : null}
    </>
  );
}
