# Matrice de vérification

Revue le 6 octobre 2026 à partir de `package.json`, des scripts et des suites.
Cette matrice indique les contrôles disponibles, pas leur dernier résultat.
Les preuves sont dans [l’état courant](../STATUS.md) et le
[registre UI](../design/redesign/03-delivery.md).

| Périmètre | Commande | Portée |
| --- | --- | --- |
| Migrations, sources, Rust, TypeScript, builds Web/Desktop | `pnpm check` | Contrôles statiques ; aucun parcours natif |
| Packages Lens | `pnpm test:packages` | Contrats et diff des packages actuels |
| Cœur local et CLI | `pnpm test:core` | Suites Rust cœur/CLI et conservation des données |
| Workspace PostgreSQL | `pnpm test:rust` | Base jetable obligatoire et intégrations natives explicites |
| Serveur seul | `pnpm test:postgres` | Serveur et intégrations natives sur base isolée |
| Shell Desktop et adaptateurs | `pnpm test:tauri` | Tests Rust Tauri, dont adaptateur Git |
| Présentation Web/Desktop | `pnpm test:e2e:renderer` | Transports contrôlés, interactions et erreurs |
| Runtime des lenses | `node node_modules/@playwright/test/cli.js test tests/e2e/lens-runtime.spec.ts` | Empreintes, isolation, délais, cache hors ligne et réponses concurrentes |
| Desktop complet | `pnpm test:desktop:ci` | Tauri réel : SCM, équipe et pont Git |
| Desktop limité | `node scripts/test-native-ui-ci.mjs desktop-partial` | Projection et synchronisation sous droits restreints |
| Collaboration Desktop ciblée | `node scripts/test-native-ui-ci.mjs desktop-team` | Appareil, revues, versions, équipe et administration |
| Web réel | `pnpm test:web:ci` | Web construit, HTTPS/PostgreSQL, création de projet, installation et affichage dynamique des lenses |
| Upgrade des migrations | `pnpm test:migrations:upgrade` | Baseline fournie par le harness CI |
| Contrat SCM | `pnpm test:scm-contracts` | Fixtures canoniques SCM |

`pnpm test` orchestre les gates statiques, packages, modèles ciblés, workspace
PostgreSQL, Tauri, renderer, Desktop complet et Web. Le scénario Desktop limité
est distinct. Les modes Web restreint (`LAYRS_NATIVE_WEB_RESTRICTED=1`) et
clients partagés (`LAYRS_NATIVE_SHARED_CLIENTS=1`) sont explicites ; vérifier
la configuration de la fixture avant de revendiquer leur couverture.

Les scénarios natifs utilisent des dossiers temporaires. La base doit être
jetable, avec `LAYRS_TEST_DATABASE_ISOLATED=1` si une URL est fournie. Ne jamais
copier ses identifiants dans les sorties ou capturer les secrets dans les
artefacts. Les échecs conservent le contexte utile au diagnostic.

Pour cibler la régression « création Web vide → clonage natif → publication »,
définir `LAYRS_TEST_EMPTY_PROJECT_CLONE_ONLY=1`, puis exécuter
`cargo test -p layrs-server --test native_clients_v3 two_native_clients_publish_clone_receive_divergence_and_local_operations -- --ignored --exact`.
Ce scénario est également inclus dans la recette native générale sans cette option.
Il utilise le vrai serveur HTTPS, PostgreSQL et les chemins annulables de Desktop.
Il couvre aussi les destinations non vides : même projet, autre projet, projet
absent du serveur, refus des fichiers ordinaires et données invalides, liaison
sans remplacement des fichiers et rejeu exact de création après réponse perdue.

`cargo test -p layrs-client-core --test native_step_diff` vérifie la lecture des
fichiers d'une capture automatique après réouverture, la conservation du checkout
et le refus des Steps ou parents étrangers. Sous Windows restreint, placer
`TEMP` et `TMP` dans un dossier temporaire accessible du dépôt.
`desktop-renderer.spec.ts` couvre l'affichage après capture et rechargement,
la distinction avec une capture reçue, les actions des dossiers occupés et les
réponses tardives après changement de destination. Ses transports sont simulés.

Windows/WebView2 est nécessaire aux recettes Tauri. Les jobs Linux prouvent
leur périmètre Rust/serveur/Web, pas les parcours Desktop Windows. Une réussite
locale ne prouve ni la CI distante, ni l’installation MSI, ni macOS/ARM.

Pour une nouvelle fonctionnalité, vérifier l’invariant Rust, le raccord de
commande et le parcours observable. Conserver les refus et protections lors
d’une refonte d’interface. Voir [le guide](testing.md) pour les prérequis.
