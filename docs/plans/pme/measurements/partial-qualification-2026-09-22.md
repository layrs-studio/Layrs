# Projection éditable et distribution — 22 septembre 2026

Sources de distribution figées au commit `6436fb0f34e7c9b883b5e69610bc876caadae592`.
Les ajouts ultérieurs de journaux/tests navigateur ne changent pas les sources
compilées. [Manifestes, empreintes et rapports](partial-artifacts-2026-09-22.json).

| Vérification | Résultat et portée |
| --- | --- |
| HTTPS/PostgreSQL intégré | PASS61,17s : clone/migration, capture/publication/reprise, namespace, réception, ACL, restauration/inverse, concurrence et Gates |
| Desktop réel Tauri/WebView2 | PASS29,8s : projet partiel, chemins réservés, collision externe, publication/réception, restauration et annulation |
| Web réel Edge autonome | PASS4,1s (recette4,9s) sur le code final, sans complément Desktop : login, historique exact de deux Steps, chemins/droits, revue, preuve CI, comptes, logout |
| Web/CLI/Desktop même instance | PASS33,4s : deux Stores et octets exacts, publications/réceptions dans les deux sens, deux Markers et Release aux mêmes cibles ; commentaire Web relu CLI, preuve exacte, Gate refusée sans nouvel événement, ACL et purge des vues refusées |
| Store3 Windows | 42 tests PASS ; Clippy strict PASS |
| Store3 Linux | Réception11 PASS, restauration6 PASS ; namespace24 PASS lors du lot précédent |
| Client natif | 41 tests PASS7,46s |
| CLI partielle | 5 tests PASS0,69s ; Clippy strict PASS46,86s |
| UI | 38 tests Node PASS ; TypeScript UI/Desktop/Web PASS ; builds production Web/Desktop PASS |
| Workspace | Clippy strict tous targets PASS35,63s ; format, taille des sources, migrations et diff vérifiés |
| Archives release Windows/Linux x86_64 | 26 fichiers par archive vérifiés ; même empreinte source et Cargo.lock |
| Smoke des archives | 8 parcours par OS PASS ; lanceur Linux extrait PASS |
| Upgrade/rollback archives4→5 | PASS Windows/volume Linux natif ; 5 Operations conservées, corruption détectée avant exécution, sauvegarde et état incident conservés |
| MSI Windows x64 | Construit, empreinte vérifiée ; aucune installation effectuée |

Les tests de parsing des previews distants CLI n'effectuent aucun accès réseau.
Les preuves HTTPS de restauration appartiennent au client natif et à Desktop.
Les anciennes tentatives de fixture échouées ne sont pas comptées comme réussies.

La recette trois-clients est activée explicitement avec
`LAYRS_NATIVE_SHARED_CLIENTS=1` et `LAYRS_NATIVE_WEB_BROWSER_CHANNEL=msedge`
avant `node scripts/test-native-ui-ci.mjs web`. Elle exige le vrai CLI construit
dans `target/debug`, les dépendances Tauri/Desktop et les variables PostgreSQL
privées habituelles. Sans ce flag, la recette Web seule conserve sa portabilité
Linux ; ne pas lui attribuer les assertions Desktop du complément Windows.
Le compte restreint voit les chemins et leurs cadenas, mais reçoit403 pour
l'historique complet ; son fetch-plan ne fournit aucune capacité de contenu.
La preuve CI de ce test est synthétique, admise par le vrai service : son
affichage est qualifié, sans prétendre exécuter le runner dans cette recette.
Le runner réel dispose de preuves séparées dans le journal T6.
Logs finaux : `tmp/web-three-clients-20260922-final.log` et
`tmp/web-standalone-20260922-final.log`. TypeScript strict des recettes PASS.

## Artefacts courants

- Windows : `dist/native-windows-qualified5-20260922/`.
- Linux : `dist/native-linux-qualified5-20260922/candidate/`.
- MSI : `dist/desktop-windows-qualified5-20260922/`.

Chaque archive contient `PARTIAL-CHECKOUTS.md`, un manifeste et un vérificateur
autonome. Les paquets qualified4 restent historiques. Les archives qualified5
déclarent `dirty:false`. Le MSI déclare `dirty:true` car journaux/tests navigateur
ont changé pendant sa fabrication ; son empreinte de sources compilées est
restée inchangée, contrôlée avant/après construction.

## Limites maintenues

Publication partielle : fichiers ordinaires, 16Mio d'octets remplacés et 1024
chemins par capture. Pas de déplacement de dossier ni de remplacement d'un
dossier physique par un fichier. Les Gates ne sont pas contournées ; le parcours
partiel ne propose pas encore l'admission de candidats puis revue/CI/intégration
d'un nouveau Step sur Layer protégée. Les copies obtenues avant révocation ne
sont pas effacées rétroactivement ; la réception conserve les anciennes données
en quarantaine.

Les upgrades testent la compatibilité du format2. La migration explicite vers
format3 est qualifiée séparément, avec conservation de la source. Aucune
conversion implicite ni migration PostgreSQL ne découle du test d'archives.

Paquets non signés, aucun push, déploiement ou installation MSI. L'avertissement
Tauri sur le type de bundle demeure ; mise à jour automatique, machine vierge,
macOS/ARM, pilote humain et CI distante ne sont pas qualifiés par ces tests.

## Logs locaux

`tmp/partial-full-20260922-combined.log`,
`tmp/desktop-partial-restore-20260922.log`,
`tmp/native-web-partial-20260922-final.log`,
`tmp/partial-client-final-20260922.log`,
`tmp/partial-clippy-workspace-20260922.log`,
`tmp/partial-cli-clippy-20260922.log`,
`tmp/package-{windows,linux,desktop}-qualified5-20260922.log`,
`tmp/smoke-{windows,linux}-qualified5-20260922.log`.

Rapports upgrade : `tmp/upgrade-windows-qualified4-to-5-20260922/report.json`
et `dist/upgrade-linux-qualified4-to5-20260922/{recipe/report.json,container.json}`.
Le manifeste JSON cité en tête conserve les résultats et hashes essentiels.
