import { PageHeader } from "@layrs/ui";
import { NativeRemotePanel } from "../NativeRemotePanel";
import { useWorkspace } from "../app/workspace";

/**
 * Server connection, publication and reception. The panel keeps its proven
 * transfer, cancellation and receipt handling until its own redesign lot.
 */
export function SyncPage() {
  const workspace = useWorkspace();
  const { data, busy } = workspace;
  return (
    <div className="layrs-page">
      <PageHeader
        title="Synchronisation"
        description="Publier envoie vos captures et attend leur acceptation par le serveur ; recevoir récupère l’état autorisé de l’équipe."
      />
      <div className="layrs-legacy sync-legacy">
        <NativeRemotePanel
          root={workspace.root}
          localSpaceId={data?.state.space.spaceId}
          opened={Boolean(data)}
          busy={Boolean(busy)}
          onBusy={(value) => workspace.setForeground(value ? "Synchronisation…" : null)}
          onChanged={() => void workspace.refresh()}
        />
      </div>
    </div>
  );
}
