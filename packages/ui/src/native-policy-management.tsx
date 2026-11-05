export type PolicyManagement = { mode: "managed" | "custom"; currentPolicySetId: string; managedPolicySetId: string };

export function NativePolicyManagement({ value }: { value: PolicyManagement | null }) {
  if (!value) return null;
  return <section aria-label="Gestion des règles d’accès">
    <p>{value.mode === "custom"
      ? "Ce projet utilise des règles d’accès personnalisées. Les modifications de droits par équipe ou par rôle sont suspendues. Un administrateur doit rétablir la politique gérée avant de les modifier ici."
      : "Les droits de ce projet sont gérés par les équipes et les rôles individuels."}</p>
    <details><summary>Références de la politique d’accès</summary>
      <p>Politique actuelle : <code>{value.currentPolicySetId}</code></p>
      <p>Politique gérée à rétablir : <code>{value.managedPolicySetId}</code></p>
    </details>
  </section>;
}
