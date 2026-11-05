import { useCallback, useEffect, useRef, useState } from "react";
import { AlertTriangle, Building2, RefreshCw } from "lucide-react";
import { Button } from "../components/Button";
import { Banner, Card, Loading } from "../components/Feedback";
import { Dialog } from "../components/Dialog";
import { SelectField, TextField } from "../components/Field";
import { describeError } from "./errors";
import type { TeamTransport } from "./types";

type Organisation = { id: string; name: string; slug?: string; projectCount: number; memberCount: number; isPrimary: boolean };
type Settings = {
  revision: number;
  stateToken: string;
  multiOrganisationEnabled: boolean;
  personalProjectsEnabled: boolean;
  primaryOrganisationId: string;
  organisations: Organisation[];
};

/** Server-authoritative, reversible mode switch with an explicit impact review. */
export function OrganisationSettingsPanel({ scope, administration }: { scope: string; administration: TeamTransport }) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [dialog, setDialog] = useState<"enable" | "disable" | null>(null);
  const [primary, setPrimary] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const generation = useRef(0);
  const load = useCallback(async () => {
    const current = ++generation.current;
    setLoading(true);
    try {
      const next = await administration<Settings>({ action: "organisationSettings" });
      if (current === generation.current) setSettings(next);
    } catch (cause) {
      if (current === generation.current) setError(describeError(cause, "La configuration des organisations n’a pas pu être lue."));
    } finally {
      if (current === generation.current) setLoading(false);
    }
  }, [administration]);
  useEffect(() => {
    setSettings(null); setDialog(null); setError(""); setNotice("");
    void load();
    return () => { generation.current++; };
  }, [scope, load]);

  const chosen = settings?.organisations.find(item => item.id === primary);
  const ambiguousName = chosen && settings!.organisations.filter(item => item.name.trim().toLocaleLowerCase("fr") === chosen.name.trim().toLocaleLowerCase("fr")).length > 1;
  const confirmationTarget = chosen ? ambiguousName ? chosen.slug ?? chosen.id : chosen.name : "";
  const affected = settings?.organisations.filter(item => item.id !== primary) ?? [];
  const open = (mode: "enable" | "disable") => {
    setPrimary(mode === "enable" ? settings?.primaryOrganisationId ?? "" : "");
    setConfirmation(""); setError(""); setNotice(""); setDialog(mode);
  };
  const save = async () => {
    if (!settings || !dialog || saving || !primary || (dialog === "disable" && (!chosen || confirmation !== confirmationTarget))) return;
    const current = generation.current;
    setSaving(true); setError("");
    try {
      const next = await administration<Settings>({ action: "configureOrganisations", request: {
        expectedRevision: settings.revision, expectedStateToken: settings.stateToken,
        multiOrganisationEnabled: dialog === "enable", primaryOrganisationId: primary, confirmDanger: dialog === "disable",
      } });
      if (current !== generation.current) return;
      setSettings(next); setDialog(null);
      setNotice(next.multiOrganisationEnabled ? "Le mode multi-organisation est activé. Les accès existants sont rétablis." : "Le mode mono-organisation est activé. Les autres organisations sont conservées et inaccessibles.");
      window.dispatchEvent(new Event("layrs-organisations-changed"));
    } catch (cause) {
      if (current !== generation.current) return;
      setDialog(null);
      setError(`${describeError(cause, "Le changement n’a pas été confirmé.")} Vérifiez l’état actualisé avant de recommencer.`);
      await load();
      window.dispatchEvent(new Event("layrs-organisations-changed"));
    } finally {
      setSaving(false);
    }
  };

  return <div className="team-panel team-panel--narrow">
    <div className="team-toolbar"><p className="team-muted">Choisissez comment les organisations sont accessibles sur cette installation.</p>
      <Button icon={<RefreshCw size={16} aria-hidden />} busy={loading} disabled={saving} onClick={() => { setError(""); setDialog(null); void load(); }}>Actualiser</Button></div>
    {error && <Banner tone="danger">{error}</Banner>}
    {notice && <Banner tone="success">{notice}</Banner>}
    {!settings ? loading && <Loading label="Lecture des organisations…" /> : <>
      <Card>
        <h2><Building2 size={20} aria-hidden /> Organisations</h2>
        <p><strong>{settings.multiOrganisationEnabled ? "Mode multi-organisation" : "Mode mono-organisation"}</strong></p>
        <p>Organisation principale : <strong>{settings.organisations.find(item => item.id === settings.primaryOrganisationId)?.name ?? "Non définie"}</strong></p>
        <p>{settings.multiOrganisationEnabled ? "Chaque personne accède aux organisations dont elle est membre." : "Seule l’organisation principale est accessible. Les autres restent conservées avec leurs projets et leurs permissions."}</p>
        <Button variant={settings.multiOrganisationEnabled ? "danger" : "primary"} disabled={loading || saving} onClick={() => open(settings.multiOrganisationEnabled ? "disable" : "enable")}>
          {settings.multiOrganisationEnabled ? "Passer en mono-organisation" : "Activer le multi-organisation"}
        </Button>
      </Card>
      <p className="team-muted">Les projets personnels sont {settings.personalProjectsEnabled ? "activés" : "désactivés"}. Cette option reste indépendante et ne change pas lors de la bascule.</p>
      <ul className="team-list" aria-label="Organisations de l’installation">{settings.organisations.map(item => <li className="team-row" key={item.id}>
        <div><strong>{item.name}</strong><p className="team-muted">{item.slug ?? item.id} · {item.projectCount} projet(s) · {item.memberCount} membre(s) · {item.isPrimary ? "Principale" : settings.multiOrganisationEnabled ? "Accessible selon les permissions" : "Inaccessible en mode mono"}</p></div>
      </li>)}</ul>
    </>}
    <Dialog open={dialog !== null} onClose={() => setDialog(null)} dismissible={!saving}
      title={dialog === "disable" ? "Attention : changement dangereux" : "Activer le multi-organisation"}
      description={dialog === "disable" ? "Les personnes travaillant dans les autres organisations perdront leur accès au serveur pour ces projets." : "Les organisations conservées redeviendront accessibles selon leurs permissions existantes."}
      footer={<><Button disabled={saving} onClick={() => setDialog(null)}>Annuler</Button><Button variant={dialog === "disable" ? "danger" : "primary"} busy={saving}
        disabled={!primary || (dialog === "disable" && (!chosen || confirmation !== confirmationTarget))} onClick={() => void save()}>
        {dialog === "disable" ? "Confirmer le passage en mono-organisation" : "Activer les organisations"}</Button></>}>
      {dialog === "disable" ? <>
        <Banner tone="danger" title="Attention : c’est dangereux"><AlertTriangle size={18} aria-hidden /> Les autres organisations et leurs projets deviendront inaccessibles jusqu’à la réactivation du mode multi-organisation. Les synchronisations et publications seront refusées. Aucun projet ne sera supprimé ou transféré ; les copies déjà téléchargées sur les ordinateurs restent présentes.</Banner>
        <SelectField label="Organisation à garder comme principale" value={primary} onChange={event => { setPrimary(event.target.value); setConfirmation(""); }} disabled={saving}>
          <option value="">Choisir une organisation…</option>{settings?.organisations.map(item => <option key={item.id} value={item.id}>{item.name} — {item.slug ?? item.id} · {item.projectCount} projet(s)</option>)}
        </SelectField>
        {chosen && <><p><strong>{chosen.name} ({chosen.slug ?? chosen.id})</strong> reste accessible avec ses permissions actuelles. Aucun membre n’est ajouté automatiquement.</p>
          <p>Organisations qui deviendront inaccessibles :</p>
          <ul aria-label="Organisations rendues inaccessibles">{affected.map(item => <li key={item.id}>{item.name} ({item.slug ?? item.id}) · {item.projectCount} projet(s) · {item.memberCount} membre(s)</li>)}</ul>
          {!affected.length && <p>Aucune autre organisation.</p>}
          <TextField label={ambiguousName ? "Recopiez l’identifiant de l’organisation principale pour confirmer" : "Recopiez le nom de l’organisation principale pour confirmer"} hint={ambiguousName ? `Plusieurs organisations portent ce nom. Saisissez ${confirmationTarget}.` : undefined} value={confirmation} onChange={event => setConfirmation(event.target.value)} autoComplete="off" disabled={saving} />
        </>}
      </> : <p>Les membres et les droits de chaque organisation sont conservés. Le sélecteur d’organisation réapparaîtra dans la navigation.</p>}
    </Dialog>
  </div>;
}
