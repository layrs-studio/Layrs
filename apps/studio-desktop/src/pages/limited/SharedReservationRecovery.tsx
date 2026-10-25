import { useState } from "react";
import { Banner, Button, Card } from "@layrs/ui";
import { useOfflineTask } from "../../app/useOfflineTask";
import { cancelledReservation, recoveryActionBlocker, reservationRecoveryRequest, type ConversionInspection } from "../../app/offline";

export function SharedReservationRecovery({ inspection, invalidate }: { inspection: ConversionInspection | null; invalidate: () => void }) {
  const task = useOfflineTask();
  const [confirmed, setConfirmed] = useState(false);
  const blocked = task.busy ? "Opération en cours." : recoveryActionBlocker(inspection, "reservation");
  async function cancel() {
    if (blocked || !inspection) return;
    invalidate();
    setConfirmed(false);
    const result = await task.execute("Annulation de la réservation de capture…", async () => cancelledReservation(await task.local(reservationRecoveryRequest(inspection))));
    if (result) setConfirmed(true);
  }
  return <section className="settings-section" aria-labelledby="shared-reservation-recovery">
    <h2 id="shared-reservation-recovery">Réservation d’une capture interrompue</h2>
    <Card className="settings-card">
      <p>Annulez la réservation de stockage laissée par une capture interrompue. Les captures acceptées et vos fichiers de travail restent conservés. Si une autre opération ou son journal en dépend, reprenez cette opération : le moteur refusera l’annulation.</p>
      <Button disabledReason={blocked} onClick={() => void cancel()}>Annuler la réservation de capture</Button>
      {confirmed && <p role="status">Réservation de capture annulée ou déjà absente, selon la vérification du moteur. Aucun quota n’a été modifié.</p>}
      {task.error && <Banner tone="danger" title="La réservation n’a pas pu être annulée.">{task.error}</Banner>}
    </Card>
  </section>;
}
