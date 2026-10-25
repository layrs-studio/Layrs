# Qualification locale L6 — 5 octobre 2026

Sources de départ : `4601b16`, branche `redesign/foundations`. Cette passe
qualifie les modifications locales décrites ci-dessous. Elle ne vaut ni
publication d’un installateur ni résultat de CI distante.

Cette preuve précède le chantier création de projets et distribution des lenses
du même jour. Son empreinte binaire décrit la livraison L6, pas les sources
postérieures ; consulter l’[état courant](../../STATUS.md) pour leurs résultats.

## Périmètre livré

Desktop utilise les composants partagés de collaboration et d’administration.
Les données viennent du transport Tauri et du serveur réel ; les projets à
accès limité gardent leurs commandes et revues spécifiques.

| Parcours | Surface actuelle | Preuve ciblée |
| --- | --- | --- |
| P01–P04, P18–P20 : entrée et navigation | Shell existant, catalogue et connexion | Renderer ; connexion réelle incluse dans les recettes natives |
| P05, P08–P11, P23 : travail local | Changements, historique, Layers, fusion, synchronisation, diagnostic | Quatre scénarios SCM réels ; recette distincte d’accès limité |
| P06–P08 : contenu distant | Contenu publié › Fichiers / Historique ; détail de capture | Recette Desktop : filtre chemin/acteur, provenance datée, comparaison avec parent |
| P12–P13b : revues et validations | Revues, version exacte, discussion et exécutions | Brouillons, correction auditée, réponse, version historique, lien de notification, preuve et annulation d’une exécution en attente |
| P14 : versions | Contenu publié › Versions | Publication et révocation avec motif |
| P15–P17, P21 : équipe et administration | Équipe, Accès aux fichiers, Réglages serveur ; administration via Équipe | Équipe/historique, invitation/révocation, politique par référence et règles, retour aux rôles, protection, webhook, comptes, stockage et disponibilité |
| P22 : Git | Import / export Git | Source modifiée après aperçu refusée ; import puis ouverture du Layer ; export bare contrôlé par Git et comparaison des octets |

Le bouton d’intégration d’une revue reste désactivé : cette page ne peut pas
lier une action de synchronisation à la capture et au Layer exacts de la revue.
La publication passe par le parcours Desktop existant après vérification de ces
références. Les aperçus de contenus distants ne sont pas raccordés.

## Corrections pendant la recette

- Chargement des panneaux partagés compatible avec le cycle de vérification
  React StrictMode ; les réponses d’un ancien contexte sont invalidées.
- Administration d’instance vérifiée par `whoami`, même quand le compte n’a
  pas accès au projet ; refus pour un compte non administrateur.
- Palette clavier : nom accessible du bouton conservé à 960 px, ouverture par
  Ctrl+K, fermeture par Échap et retour du focus.
- Aperçus Git bornés et expirables, liés au dossier et à son état ; contrôle
  de l’état attendu dans la préparation effective du pont pour fermer la fenêtre
  de concurrence entre vérification et exécution.
- Identité attendue vérifiée avec le même jeton que la requête native, même
  après changement de compte depuis un autre processus.
- Refus HTTP et refus locaux avant envoi distingués d’une réponse perdue ;
  incertitude conservée par demande exacte après fermeture ou rechargement.
  Le marqueur local ne contient ni jeton ni document de politique.
- Reprise explicite d’une demande de politique depuis le journal natif, sans
  recalculer l’opération à partir des règles déjà modifiées ; libération de
  l’état d’attente sur reçu appliqué comme sur reçu refusé.

## Résultats

| Contrôle exécuté le 5 octobre | Résultat observé |
| --- | --- |
| `pnpm check` | Réussi : 19 migrations immuables, taille des sources, formatage Rust, Clippy workspace, TypeScript, builds Web/Desktop |
| `pnpm test:e2e:renderer` | 27/27 : navigation, travail local, garde d’accès limité, palette et thèmes, administration indépendante du projet, rotation de compte simulée, refus, rechargement et récupération exacte d’une politique appliquée sans réponse |
| Desktop SCM réel | 4/4 : captures/restauration, brouillons partiels, variantes canoniques et conflit de renommage |
| Desktop équipe réel | 1/1 : parcours détaillé dans la matrice ci-dessus, HTTPS et PostgreSQL jetable |
| Desktop Git réel | 1/1 : aperçu périmé, import, Layer importé, export, contenu et `git fsck --full --no-reflogs` |
| Desktop limité réel | 1/1 : migration, chemins réservés, capture et publication sous droits restreints |
| Web réel reconstruit | 1/1 : identité, revues/preuves, refus de publication, administration, invitations et politique de fichiers |
| Adaptateur Git Rust | 2/2 : préconditions de l’aperçu et exécution |
| Identité native Rust, module credentials | 6/6 : formats de requête, principal attendu et refus avant réseau lorsque le coffre ne contient plus le credential |
| Pont Git, vérification indépendante ciblée | 8/8 : import/reprise, conservation des données, export/reprise et formats incompatibles ; injection de 15 points de panne à l’import |

Les quatre scénarios SCM, Git et équipe ont été exécutés au cours de cette
passe, en plusieurs commandes : ce tableau n’affirme pas une exécution unique
verte de `pnpm test`. La garde de compte a un test renderer de rotation simulée
et un test unitaire Rust ; la rotation entre deux processus réels n’a pas été
reproduite automatiquement. Les reprises de droits après réponse perdue sont
testées dans les vrais composants avec transport contrôlé, pour une politique
enregistrée, un ajout de règle de fichier et un reçu de refus autoritatif.

Le build Vite Desktop signale un bundle principal supérieur à 500 ko minifié,
sans faire échouer la compilation. Ce n’est pas une mesure de performance.

Les captures locales sont conservées sous `tmp/l6-visual/` (Git natif, réglages
natifs, Git compact clair/sombre). Ce dossier de recette est ignoré par Git.
Les captures ont été inspectées ; elles ne constituent pas un audit exhaustif
de contraste ou de toutes les tailles d’écran.

Les constats des revues indépendantes de l’intégration, du pont Git et des
frontières d’identité/reprise ont été corrigés. La dernière relecture clôt le
cas du reçu refusé ; les scénarios de composant correspondants ont
réussi dans la suite renderer complète. Ces revues ciblées ne constituent pas
un audit exhaustif du produit.

## Binaire local de recette

Après reconstruction des interfaces, commande réussie :

```text
cargo build --offline --manifest-path apps/studio-desktop/src-tauri/Cargo.toml --features custom-protocol
```

Binaire Windows x64 debug avec interface embarquée :
`apps/studio-desktop/src-tauri/target/debug/layrs-studio-desktop.exe`
(58 105 856 octets). SHA-256 de cette construction :

```text
32fa82d33b64c92b675c8de5ee5fa23b482bc0692bc1fc1a2db0e7c4c353919b
```

Ce binaire est construit, sans MSI ni signature. Les parcours automatisés
utilisent la construction Tauri de recette dans `target/studio-desktop-e2e`,
et non une installation de cet exécutable sur une machine vierge.

## Reproduire

Utiliser les commandes du [guide de tests](../../development/testing.md) avec
une base PostgreSQL jetable explicitement isolée. Les recettes natives démarrent
un serveur HTTPS et la vraie application Tauri. Elles ne doivent pas tourner
simultanément : leur nettoyage peut arrêter une autre instance de recette.

`pnpm test:e2e:renderer` utilise les fixtures de rendu et ne qualifie pas le
transport natif. Les captures renderer sont des données fictives ; les captures
natives doivent éviter les dialogues révélant des secrets.

## Limites de qualification

Windows x64, WebView2 et Chromium locaux ; PostgreSQL dans Docker Linux.
Pas de validation macOS/ARM, de pilote humain, d’installation sur machine
vierge ou de signature MSI dans cette passe. Contraste exhaustif, budgets de
performance et [qualifications externes](../../plans/pme/REMAINING.md) restent
ouverts. L’export Git produit un dépôt bare neuf ; une exportation interrompue
se reprend par le CLI. Les limitations de formats du pont restent applicables.
