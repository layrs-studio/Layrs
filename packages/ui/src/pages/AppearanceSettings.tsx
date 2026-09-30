import { Banner, Card, Kbd } from "../components/Feedback";
import { Switch } from "../components/Field";
import { useAppearance } from "../foundation/appearance";
import { PageHeader } from "../shell/AppShell";
import { ThemeSwitcher } from "../shell/ThemeSwitcher";

/** P19: personal, local preferences. Changes apply immediately, without a save button. */
export function AppearanceSettings({ surface }: { surface: "web" | "desktop" }) {
  const { appearance, persisted, setAppearance } = useAppearance();
  const osReduced = typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
  const mod = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘" : "Ctrl";
  return (
    <div className="layrs-page layrs-page--narrow">
      <PageHeader
        title="Apparence"
        description={
          surface === "web"
            ? "Ces réglages sont propres à ce navigateur. Ils ne modifient ni l’équipe ni les projets."
            : "Ces réglages sont propres à cet ordinateur. Ils ne modifient ni l’équipe ni les projets."
        }
      />
      {!persisted && (
        <Banner tone="warning" title="Préférence non mémorisée">
          Le stockage local est indisponible : votre choix s’applique jusqu’à la fermeture de Layrs.
        </Banner>
      )}
      <Card className="settings-card">
        <div className="settings-row">
          <div>
            <p className="settings-row__title">Thème</p>
            <p className="settings-row__hint">« Système » suit le réglage de votre ordinateur, même quand il change.</p>
          </div>
          <ThemeSwitcher size="md" />
        </div>
        <div className="settings-row">
          <Switch
            label="Réduire les animations"
            checked={appearance.reduceMotion || osReduced}
            disabled={osReduced}
            onChange={(reduceMotion) => setAppearance({ reduceMotion })}
            hint={
              osReduced
                ? "Votre système demande déjà moins d’animations ; Layrs le respecte."
                : "Supprime les transitions décoratives. Les indicateurs de progression restent visibles."
            }
          />
        </div>
        <div className="settings-row">
          <div>
            <p className="settings-row__title">Taille du texte</p>
            <p className="settings-row__hint">
              Utilisez le zoom : <Kbd>{mod}</Kbd> <Kbd>+</Kbd> / <Kbd>{mod}</Kbd> <Kbd>−</Kbd>.
            </p>
          </div>
        </div>
      </Card>
      <section className="settings-section" aria-labelledby="shortcuts-title">
        <h2 id="shortcuts-title">Raccourcis clavier</h2>
        <Card className="settings-card">
          <dl className="shortcut-list">
            <dt>
              <Kbd>{mod}</Kbd> <Kbd>K</Kbd>
            </dt>
            <dd>Rechercher un projet, une page ou une action</dd>
            <dt>
              <Kbd>{mod}</Kbd> <Kbd>Entrée</Kbd>
            </dt>
            <dd>Valider le formulaire principal de la page, s’il est complet</dd>
            <dt>
              <Kbd>Échap</Kbd>
            </dt>
            <dd>Fermer une fenêtre ou un panneau</dd>
            <dt>
              <Kbd>↑</Kbd> <Kbd>↓</Kbd>
            </dt>
            <dd>Parcourir la liste active</dd>
          </dl>
        </Card>
      </section>
    </div>
  );
}
