import { ConfirmModal } from "@layrs/ui";
import type { LocalLayerSummary, LocalSpaceSummary } from "./tauri";

interface DesktopConfirmationsProps {
  activeParentLayer: LocalLayerSummary | null;
  clearStepsTarget: LocalLayerSummary | null;
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
  clearStepsTarget,
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
  return (
    <>
      <ConfirmModal
        confirmLabel="Forget local"
        danger
        description={
          <p>
            Layrs will keep the project files, archive local .layrs metadata, and disconnect this folder from Studio so
            it can be pulled again.
          </p>
        }
        disabled={!forgetTarget}
        onCancel={onCancelForget}
        onConfirm={() => forgetTarget && onConfirmForget(forgetTarget.localSpaceId)}
        open={Boolean(forgetTarget)}
        title={`Forget ${forgetTarget?.name ?? "Local Space"}`}
      />
      <ConfirmModal
        confirmLabel="Delete Layer"
        danger
        description={<p>Deleting a Layer removes its local Layer state. Keep this action away from receive and publish.</p>}
        disabled={!deleteLayerTarget}
        onCancel={onCancelDeleteLayer}
        onConfirm={() => deleteLayerTarget && onConfirmDeleteLayer(deleteLayerTarget.layerId)}
        open={Boolean(deleteLayerTarget)}
        title={`Delete ${deleteLayerTarget?.displayName ?? "Layer"}`}
      />
      <ConfirmModal
        confirmLabel="Disconnect Layer"
        danger
        description={
          <p>
            Future Steps from the parent Layer will no longer flow into this Layer automatically. Existing files and
            Steps stay in place.
          </p>
        }
        disabled={!disconnectLayerTarget}
        onCancel={onCancelDisconnectLayer}
        onConfirm={() => disconnectLayerTarget && onConfirmDisconnectLayer(disconnectLayerTarget.layerId)}
        open={Boolean(disconnectLayerTarget)}
        title={`Disconnect ${disconnectLayerTarget?.displayName ?? "Layer"} from parent`}
      />
      <ConfirmModal
        confirmLabel="Clear Steps"
        danger
        description={
          <p>
            Layrs will remove this Layer history from the active timeline and archive the Step metadata. The files in the
            folder and object store are kept.
          </p>
        }
        disabled={!clearStepsTarget}
        onCancel={onCancelClearSteps}
        onConfirm={() => clearStepsTarget && onConfirmClearSteps(clearStepsTarget.layerId)}
        open={Boolean(clearStepsTarget)}
        title={`Clear Steps from ${clearStepsTarget?.displayName ?? "Layer"}`}
      />
      <ConfirmModal
        confirmLabel="Weave to parent"
        description={
          <p>
            Layrs will move the current Layer Steps into parent Layer {activeParentLayer?.displayName ?? "parent"} using
            the durable Weave flow. If conflicts appear, you can resolve or abort from the Weaves page.
          </p>
        }
        disabled={!selectedLayer || !activeParentLayer}
        onCancel={onCancelWeaveParent}
        onConfirm={onConfirmWeaveParent}
        open={confirmWeaveParent}
        title={`Weave ${selectedLayer?.displayName ?? "Layer"} to ${activeParentLayer?.displayName ?? "parent"}`}
      />
    </>
  );
}
