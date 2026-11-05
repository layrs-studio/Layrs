export type NativeMetrics = {
  schemaVersion: "layrs.telemetry.v1"; scope: "current_process"; observedUnixMs: number; uptimeSeconds: number;
  http: { inFlight: number; completed: number; interruptedBeforeHeaders: number; headersLatencyMicrosecondsTotal: number;
    statusClasses: { class: string; count: number }[]; headersLatencyBuckets: { lessOrEqualMs: number; count: number }[] };
  readiness: { checks: number; databaseFailures: number; casFailures: number };
};
/** Project only known counters, never arbitrary response fields or error text. */
export function nativeMetrics(value: unknown): NativeMetrics {
  const object = (input: unknown): Record<string, unknown> => {
    if (!input || typeof input !== "object" || Array.isArray(input)) throw new Error("Métriques invalides");
    return input as Record<string, unknown>;
  };
  const count = (input: unknown): number => {
    if (typeof input !== "number" || !Number.isSafeInteger(input) || input < 0) throw new Error("Compteur invalide");
    return input;
  };
  const body = object(value), http = object(body.http), readiness = object(body.readiness);
  if (body.schemaVersion !== "layrs.telemetry.v1" || body.scope !== "current_process") throw new Error("Version de métriques incompatible");
  if (!Array.isArray(http.statusClasses) || http.statusClasses.length !== 5 || !Array.isArray(http.headersLatencyBuckets) || http.headersLatencyBuckets.length !== 6) throw new Error("Compteurs incomplets");
  const bounds = [5, 25, 100, 500, 2000, 10000];
  const observedUnixMs = count(body.observedUnixMs);
  if (observedUnixMs > 8_640_000_000_000_000) throw new Error("Date invalide");
  return {
    schemaVersion: "layrs.telemetry.v1", scope: "current_process", observedUnixMs, uptimeSeconds: count(body.uptimeSeconds),
    http: {
      inFlight: count(http.inFlight), completed: count(http.completed), interruptedBeforeHeaders: count(http.interruptedBeforeHeaders), headersLatencyMicrosecondsTotal: count(http.headersLatencyMicrosecondsTotal),
      statusClasses: http.statusClasses.map((input, index) => { const row = object(input); if (row.class !== `${index + 1}xx`) throw new Error("Classe HTTP invalide"); return { class: row.class as string, count: count(row.count) }; }),
      headersLatencyBuckets: http.headersLatencyBuckets.map((input, index) => { const row = object(input); if (row.lessOrEqualMs !== bounds[index]) throw new Error("Borne de latence invalide"); return { lessOrEqualMs: bounds[index], count: count(row.count) }; }),
    },
    readiness: { checks: count(readiness.checks), databaseFailures: count(readiness.databaseFailures), casFailures: count(readiness.casFailures) },
  };
}
