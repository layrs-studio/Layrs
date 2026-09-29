import { Banner, Button, PageHeader, StatusDot } from "@layrs/ui";
import { NativeDiagnosticsPanel } from "../NativeDiagnosticsPanel";
import { useWorkspace } from "../app/workspace";

export function HealthPage() {
  const workspace = useWorkspace();
  const { data, busy } = workspace;
  return (
    <div className="layrs-page layrs-page--narrow">
      <PageHeader
        title="Diagnostic et reprise"
        description="État local de ce projet, guidé par le journal. Aucune réparation destructrice n’est lancée automatiquement."
        aside={
          data && (
            <StatusDot tone={data.needsRepair ? "danger" : "success"}>{data.needsRepair ? "Reprise nécessaire" : "Aucune opération interrompue"}</StatusDot>
          )
        }
      />
      {data?.needsRepair && (
        <Banner
          tone="danger"
          title="Une écriture a été interrompue"
          actions={
            <Button variant="primary" busy={busy === "Reprise…"} busyLabel="Reprise…" disabledReason={busy && busy !== "Reprise…" ? "Opération en cours" : undefined} onClick={() => void workspace.run({ action: "recover" }, "Reprise…")}>
              Reprendre après interruption
            </Button>
          }
        >
          Les sauvegardes sont conservées. La reprise termine ou annule l’opération selon le journal.
        </Banner>
      )}
      <div className="layrs-legacy">
        <NativeDiagnosticsPanel root={workspace.root} disabled={Boolean(busy)} />
      </div>
    </div>
  );
}
