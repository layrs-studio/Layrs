# Distribution native Windows et Linux

Les archives produites ici sont des builds locaux non signés. Leur SHA-256 détecte une corruption ; il n'authentifie pas l'éditeur. Aucun certificat de signature, publication automatique ou mise à jour silencieuse n'est configuré. Obtenir l'archive et sa somme de contrôle depuis un canal de confiance administré par votre équipe.

## Construction et installation

Prérequis : Git, Node 22+, Rust défini par `rust-toolchain.toml` et `tar`. Sous Windows, installer les outils de compilation MSVC ; sous Linux, un compilateur C et les en-têtes de développement habituels. Les dépendances Rust sont verrouillées. La machine de construction doit être de la plateforme cible : cette recette ne fait pas de compilation croisée.

Depuis la racine du dépôt, dans PowerShell ou un shell Linux :

```text
node scripts/package-native.mjs --output dist/native-candidate
```

Avant chaque archive CLI/serveur et chaque MSI, le script exécute le test réel `wasm_host::tests::real_compiled_guest_has_no_imports_and_executes_deterministically` avec `--exact` et exige exactement un test passé. Cette garde vérifie le module embarqué, son digest, l'absence d'imports et sa compatibilité avec la requête/pin actuelle. Elle ne réécrit aucun guest ni manifeste : en cas d'échec, auditer/reconstruire explicitement le guest, puis reprendre la livraison dans un nouveau dossier. La commande isolée est `node scripts/check-packaged-wasm.mjs --offline` lorsque les dépendances sont déjà disponibles.

Le répertoire doit être neuf. `--offline` impose un cache Cargo déjà rempli. Le profil normal est `release` ; `--profile dev` sert aux essais et reste identifié comme tel dans le nom et le manifeste. La commande compile les sources courantes, refuse un binaire absent, copie uniquement les fichiers explicitement autorisés, vérifie leur contenu puis crée une archive `tar.gz` et `SHA256SUMS`. Elle n'inclut aucune configuration, base, clé ou donnée de travail. Un échec peut laisser un dossier incomplet : ne pas le distribuer, recommencer dans un autre dossier.

Sur Linux, vérifier `sha256sum -c SHA256SUMS`. Sur Windows, comparer `Get-FileHash -Algorithm SHA256 CHEMIN_ARCHIVE` à `SHA256SUMS`. Extraire dans un dossier neuf, puis exécuter :

```text
node CHEMIN_EXTRAIT/verify-native-package.mjs CHEMIN_EXTRAIT
```

Le manifeste associe chaque fichier à ses octets et SHA-256, et indique le commit, l'état modifié du checkout, les digests de l'arbre Rust et de Cargo.lock, le compilateur et la cible. Une modification des sources pendant la compilation fait refuser l'archive. Un checkout modifié n'est pas reproductible depuis le seul commit : conserver également les sources exactes. Les builds de cette phase restent en version applicative `0.0.0` ; le commit et les digests identifient l'artefact. Un manifeste ou une somme provenant d'une source inconnue ne suffit pas à établir la confiance.

Les guides d'exploitation voisins sont inclus dans l'archive, notamment sauvegarde, récupération, quotas et nettoyage. Leurs liens vers les sources, les recettes de test et les fichiers Compose renvoient au checkout correspondant ; ces fichiers de développement ne font pas partie de l'archive de binaires.

Ajouter le dossier vérifié au PATH du seul utilisateur ou lancer `layrs` par chemin absolu. Commencer avec `layrs --help`, puis un projet jetable `layrs init`. Aucun service système n'est installé automatiquement. `layrs-server` demande la configuration décrite dans le runbook ; les opérations de sauvegarde nécessitent aussi les utilitaires PostgreSQL compatibles. `layrs-runner` demande Docker et un token de service passé par fichier privé, jamais par argument.

`node scripts/smoke-native-package.mjs DOSSIER_EXTRAIT` vérifie le manifeste puis exerce les exécutables distribués dans un projet jetable : capture binaire, restore, undo, Layer divergents et preview Weave. La configuration utilisateur est isolée et supprimée avec le fixture. Sous Windows, cette recette utilise le worker archivé avec son digest attendu. Sous Linux x86_64, elle utilise aussi le worker archivé et vérifie une fusion texte supervisée avec les octets exacts des deux modifications. Pour construire sous Linux depuis un poste Windows, `scripts/package-native.Dockerfile` fournit Rust/Node : monter les sources en lecture seule dans `/src`, une sortie vide dans `/out`, et des volumes Cargo distincts. Les recettes Linux locales utilisent Debian Bookworm/glibc ; Alpine/musl et les machines vierges sans runtimes adéquats ne sont pas qualifiés par ces archives.

## Plateformes et Lens

| Composant | Windows x64 | Linux x64 | macOS |
|---|---|---|---|
| CLI native, serveur, runner | Construction et tests locaux | Construction et tests dans Linux Docker | Non qualifié dans cette recette |
| Checkout de liens symboliques | Refus explicite avant écriture | Recette réelle passée | Non qualifié |
| Lens WASM supervisée | Worker séparé, Job Object et digest épinglé | Worker séparé, seccomp/RLIMIT et digest épinglé | Non qualifié |
| Nettoyage CAS hors ligne | Inventaire seulement ; mutations refusées | Plan, quarantaine, reprise, restauration et purge selon [RETENTION.md](RETENTION.md) | Non qualifié |
| Desktop | Tauri, recette UI native Windows séparée | Expérimental, non livré ici | Non qualifié |

L'archive Windows inclut `layrs-lens-worker.exe` et le lanceur `layrs-with-lenses.ps1`. Celui-ci fixe son chemin absolu et son SHA-256 attendu avant de lancer la CLI. Pour Desktop, l'administrateur fixe les mêmes variables `LAYRS_LENS_WORKER` et `LAYRS_LENS_WORKER_SHA256` avant son lancement ; le digest doit commencer par `sha256:`. Aucun projet ni serveur distant ne peut choisir ce programme. Le module `native-lenses-v1.wasm` est également fourni pour inspection, mais le worker exécute sa copie embarquée. Modifier le fichier adjacent ne remplace pas son implémentation. L'archive Linux inclut `layrs-lens-worker` et le lanceur exécutable `layrs-with-lenses`, qui fixe de la même manière chemin absolu et digest. Le noyau doit fournir les primitives d'isolation qualifiées (seccomp, memfd scellé, limites de ressources) ; leur absence bloque explicitement. Sans configuration du worker, les parcours structurels et les variantes conservées restent disponibles.

Les installateurs Desktop Windows sont produits séparément par `node scripts/package-desktop-windows.mjs --output dist/desktop-candidate`, après installation des dépendances avec le lockfile pnpm. La recette utilise Vite/Tauri du workspace, compile le MSI, refuse un résultat absent ou ancien, puis fournit un manifeste et SHA256SUMS. WiX peut être téléchargé par Tauri au premier build ; aucune installation du MSI n'est effectuée. Le workflow Trust gates conserve ces artefacts. La validation de l'installation/désinstallation et la signature éditeur restent distinctes de la recette UI de l'exécutable. Aucun updater automatique n'est livré. Ne pas annoncer macOS ou Linux Desktop comme supportés sans leurs recettes natives.

La matrice `native-packaging.yml` construit et conserve les archives Windows/Linux et exécute les tests locaux de sécurité correspondants. Une définition de workflow n'est pas une exécution CI réussie : consulter les résultats du run avant distribution. Aucun upload vers une release publique n'est effectué.
