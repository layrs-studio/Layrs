# Parcours natifs actuellement raccordés

État de développement : 20 septembre 2026. Les limites et validations détaillées se trouvent dans [les journaux PME](plans/pme/REPRISE.md). Ce document décrit les commandes disponibles ; il ne constitue pas une annonce de disponibilité générale.

## Projet local

Dans un dossier existant :

```text
layrs init "Mon projet"
layrs status
layrs step
layrs diff
layrs marker create "avant-refonte"
layrs layer create "essai"
layrs layer use "essai"
```

La capture conserve les octets, les fichiers vides et les suppressions. Les dossiers `.layrs` et `.git` sont réservés et exclus. Les changements de Layer et restaurations capturent d'abord le travail courant. Une capture sans changement ne crée pas d'Operation. Les captures automatiques fonctionnent pendant l'ouverture du projet dans Desktop, après une période de calme ; elles ne constituent pas un service permanent lorsque Desktop est fermé.

Le fichier racine `.layrsignore` ignore les fichiers non suivis : une règle par ligne, commentaires `#`, jokers `*` et `?`, composant `**`, suffixe `/` pour les dossiers, préfixe `/` pour ancrer à la racine et `!` pour réinclure. Les règles sont appliquées dans l'ordre ; la dernière correspondance prévaut. Les motifs avec `/` sont relatifs à la racine. Les classes `[...]` et les échappements `\` sont refusés explicitement. Le fichier est limité à 64 KiB UTF-8 et 512 règles ; il reste toujours capturé. Un fichier déjà suivi reste suivi même si une règle l'exclut. Une restauration qui écraserait un fichier ignoré non suivi est refusée avant écriture.

Le chemin peut être sélectionné avec `--space DOSSIER`. `--json` retourne une enveloppe structurée. Le format disque natif courant est la version 2 ; un format incompatible est refusé sans conversion ni suppression. Conserver les anciens stores pour une reprise/conversion explicite.

## Restaurer et explorer

```text
layrs restore @marker:avant-refonte --path src/main.rs
layrs undo
layrs redo
layrs timeline --limit 100
layrs restore-view operation:IDENTIFIANT
layrs history @layer:main --path src/main.rs --limit 30
layrs operation-history --search capture --since 2026-09-01T00:00:00Z --limit 30
layrs release create 1.0.0
```

Sélecteurs : ID `blake3:…`, nom exact `@layer:…`, `@marker:…` ou `@release:…`. Aucun choix implicite parmi des doublons ou plusieurs têtes. Une page d'historique fournit un curseur JSON réutilisable avec `--cursor` ; il reste ancré sur son Step initial pendant de nouvelles captures. Le filtre chemin suit le chemin exact et ne déduit pas les renommages.

Undo/redo et restauration de View ajoutent des Operations ; l'histoire n'est pas supprimée. Après un nouveau travail, `redo OPERATION_ID` exige une cible explicite. Desktop propose les aperçus avant restauration et transformation. Si un journal de matérialisation reste ouvert après panne, `status` signale `needsRepair` et `layrs recover` vérifie les fichiers puis restaure les originaux ou finalise le résultat déjà commité. Une modification extérieure ambiguë bloque la réparation et conserve les sauvegardes.

## Rejouer, inverser et regrouper

```text
layrs replay @marker:changement --preview
layrs replay @marker:changement
layrs revert blake3:IDENTIFIANT --preview
layrs revert blake3:IDENTIFIANT
layrs squash blake3:PREMIERE_CAPTURE --preview
```

Pour un Step à plusieurs parents, replay/revert exige `--mainline STEP_PARENT`. Squash accepte une plage linéaire contiguë de la première capture jusqu'à la tête active. Les points nommés existants conservent leurs cibles. Une réécriture conflictuelle crée un descendant qui conserve les variantes. Le checkout garde sa projection précédente avec un statut conflictuel jusqu'à résolution explicite, comme pour une fusion.

## Fusion et résolution

```text
layrs weave essai --target main --preview
layrs weave essai --target main
layrs weave conflicts --json
layrs resolve-term CONFLICT_ID TERM_ID --layer main --preview
layrs resolve-term CONFLICT_ID TERM_ID --layer main
layrs resolve-file CONFLICT_ID CHEMIN_DU_FICHIER_FINAL --layer main --preview
layrs resolve-file CONFLICT_ID CHEMIN_DU_FICHIER_FINAL --layer main
```

Le résultat contient les deux ascendances et un reçu Weave atomique. Les conflits restent des objets avec leurs variantes identifiées ; choisissez le `termId` d'un élément `document.sides` du conflit affiché. La résolution crée un descendant et conserve le conflit original dans l'histoire. Une résolution partielle conserve les fichiers de la projection précédente ; la résolution complète installe les fichiers résultants. Desktop expose le même aperçu et le choix des variantes. Une fusion sémantique exécutée sans sandbox complète n'est pas acceptée.

Les aperçus de fusion et de réécriture sont sans écriture. Si le checkout contient des modifications, faites `layrs step` avant de demander l'aperçu. L'application d'un aperçu Desktop devenu périmé est refusée sans capturer de nouvel état.

Desktop peut lire le texte d'une variante (UTF-8, sans NUL, au plus 1 MiB), le copier dans un brouillon et prévisualiser un contenu final manuel. Les brouillons restent dans l'écran, séparés du conflit immuable. `resolve-file` accepte les octets exacts d'un fichier texte ou binaire, jusqu'à 64 MiB et dans les budgets globaux de résolution. Enregistrez ce fichier hors du checkout conflictuel pour garder sa projection intacte ; l'application refuse les modifications concurrentes. Un remplacement vide crée un fichier vide ; la suppression se choisit via une variante absente. `--executable` s'applique aux plateformes Unix capables de matérialiser ce mode.

Sous Windows et Linux x86_64, CLI et Desktop utilisent la Lens WASM si leur environnement de lancement définit `LAYRS_LENS_WORKER` (chemin absolu du binaire installé `layrs-lens-worker`, suffixe .exe sous Windows) et `LAYRS_LENS_WORKER_SHA256` (empreinte `sha256:…` issue de la distribution approuvée). Ces valeurs ne proviennent jamais du dépôt. Le binaire et ses parents restent épinglés pendant l'exécution ; mémoire, durée, carburant WASM et absence d'import sont imposés. Une configuration incomplète ou une empreinte incorrecte bloque avant capture. Sans configuration, la fusion structurelle conserve les variantes conflictuelles.

## Serveur HTTPS

Installer une instance avec [le guide self-host](../deploy/selfhost/README.md). La même origine sert l'API et Studio Web. CLI et Desktop gardent leurs jetons dans le coffre du système, isolés par origine HTTPS ; Studio Web les garde uniquement en mémoire.

```text
layrs remote login --endpoint https://layrs.example.com
layrs remote provision equipe mon-projet --endpoint https://layrs.example.com
layrs remote publish --endpoint https://layrs.example.com
layrs remote receive --endpoint https://layrs.example.com
layrs remote clone space:IDENTIFIANT DOSSIER_VIDE --endpoint https://layrs.example.com
layrs remote logout --endpoint https://layrs.example.com
```

`login` affiche un code à approuver sur la page HTTPS de l'instance. `LAYRS_SERVER_ENDPOINT` évite de répéter `--endpoint`. Une autorité privée peut être fournie avec `--ca certificat.pem` ; la vérification du certificat reste activée. Le dossier de clonage doit déjà exister et être vide.

Une publication dont la réponse a été perdue reste inscrite localement : relancer la publication réutilise sa transaction. `publish --integrate` conserve les têtes concurrentes, sans choisir arbitrairement un contenu. Les Operations et chemins Worktree restent locaux. Layers, markers et releases sont publiés avec leurs identités natives. Les protections et droits sont vérifiés par le serveur, y compris pour les appels directs à l'API.

Desktop et Studio Web exposent les propositions de revue, versions, commentaires, approbations et demandes de modification. Les décisions se rapportent à la version exacte ; une nouvelle proposition ne récupère pas automatiquement les anciennes approbations. Le panneau CI lit les exécutions et preuves produites par le service.

## Publier et révoquer une version

Le manifeste Release et sa capture restent immuables. La publication puis la révocation créent des événements chaînés, affichés dans Desktop et Studio Web. Le rôle `release_manager` peut recevoir ces droits sans obtenir les autres droits d’écriture du projet.

```text
layrs remote releases --endpoint https://layrs.example.com
layrs remote release-events RELEASE_ID --endpoint https://layrs.example.com
layrs remote release-publish RELEASE_ID --request-id IDENTIFIANT_UNIQUE_16_CARACTERES --expected-last none --endpoint https://layrs.example.com
layrs remote release-revoke RELEASE_ID PUBLICATION_ID --request-id AUTRE_IDENTIFIANT_UNIQUE --expected-last EVENT_OBJECT_ID --reason "Version retirée" --endpoint https://layrs.example.com
```

Recopiez `lastEventObjectId` fourni par `release-events` dans `--expected-last` (`none` pour la première publication). Après une réponse perdue, rejouez exactement la même requête avec le même identifiant. Un événement concurrent exige une nouvelle lecture ; les protections et preuves CI de la capture exacte sont revérifiées au moment de publier. Une révocation n’efface ni le manifeste ni les preuves historiques.

## Vérifier l’intégrité locale

`layrs doctor --json` inspecte le store en lecture seule, même lorsque l’ouverture habituelle échoue. Desktop propose la même action dans « Diagnostic du projet », sous le choix du dossier. Le rapport contient les objets/lignes vérifiés, les racines de rétention et les anomalies identifiées. Aucune réparation implicite : conservez le dépôt et ses preuves avant toute intervention.

La CLI retourne 0 pour un diagnostic complet sans erreur, 1 pour une corruption, une inspection incomplète ou une erreur d’exécution. Un rapport d’intégrité valide peut donc avoir `ok: true` et `data.clean: false` : le diagnostic a fonctionné, mais le dépôt présente des erreurs. `layrs recover` reste la reprise explicite d’une écriture interrompue.

## Identité et recherche locales

```text
layrs identity --actor actor:alice --device device:portable --name Alice
layrs identity
layrs history @layer:main --actor actor:alice --since 2026-09-01T00:00:00Z --until 2026-10-01T00:00:00Z
layrs operation-history --actor actor:alice
```

L’identité est une déclaration locale explicite pour les prochaines captures manuelles/automatiques. Elle n’accorde aucun droit serveur et ne réattribue pas le passé. Desktop permet de renseigner le nom et génère des identifiants stables pour ce dépôt. Les filtres Step utilisent les observations locales, séparées du Step anonyme ; acteur et période doivent correspondre à une même observation. Sans observation correspondante, le Step ne satisfait pas le filtre.

Chaque page fige la racine et un instantané des observations : une observation ajoutée après la première page, même antidatée, n’y entre pas. Un changement de filtre exige une nouvelle recherche. Desktop regroupe visuellement les captures automatiques successives ; chaque capture reste consultable individuellement. Les observations locales ne sont pas synchronisées comme preuve d’auteur.

## Passerelle Git : commandes

```text
layrs git import CHEMIN_GIT --dry-run
layrs git import CHEMIN_GIT
layrs git export NOUVEAU_DEPOT_BARE
layrs git export NOUVEAU_DEPOT_BARE --snapshot blake3:IDENTIFIANT
layrs git export DEPOT_INTERROMPU --resume
```

L'import conserve une provenance explicite et ne remplace pas le checkout courant. L'export exige une nouvelle destination ; ses objets sont vérifiés avec `git fsck`. Les captures conflictuelles et signatures non transférables sont signalées explicitement. Voir [la matrice de correspondance](../crates/layrs-git-bridge/README.md).

La progression est écrite sur stderr ; stdout conserve l'enveloppe JSON. `--resume` exige la destination et le plan exacts de l'export interrompu ; il ne transforme pas un dépôt arbitraire en destination autorisée. Les filtres `operation-history --actor ACTOR_ID --since RFC3339 --until RFC3339` portent sur les Operations locales. Les bornes temporelles sont inclusives et comparent les instants, indépendamment du fuseau ; les Steps anonymes ne reçoivent pas de date artificielle.

Le scan/export Git dispose de 60 secondes par défaut, réglables par `--timeout-seconds N` (1 à 86 400). L’export de 2 000 commits a été mesuré à 15,246 secondes après passage au format pack Git ; la mesure précédente de 278,687 secondes reste conservée dans T7. Ces mesures synthétiques ne garantissent pas un délai pour tout dépôt. L'installation atomique native après le scan possède ses propres limites d'admission et n'est pas couverte par ce délai Git.

## Limites encore ouvertes

Les mesures de temps, mémoire et concurrence sont consignées dans
[la recette de volumes](plans/pme/measurements/T8-local-volumes.md), avec leur
configuration et leurs limites.

- Changements fichier/dossier et symlinks Linux matérialisés avec sauvegarde et reprise, y compris après interruption. Modifications de symlinks refusées sous Windows/macOS ; bit exécutable non matérialisable sous Windows.
- Capture et restauration en flux bornées à 100 000 entrées, dossiers compris, 1 GiB par fichier et 4 GiB de contenu par projection. Les parcours locaux de 1 GiB sont qualifiés Windows/Linux ; les parcours HTTPS complets de 100 MiB et 1 GiB sont qualifiés sous Windows, avec modification incrémentale et restauration exactes. Mesures et limites dans les journaux T4/T8 ; les temps observés ne garantissent pas ceux d'une installation en production.
- Les budgets de vérification du contenu historique restent bornés : 4 GiB logiques par parcours, 256 MiB de métadonnées, 64 MiB par objet. Les versions distinctes et reconstructions répétées consomment ce budget même si leurs chunks sont dédupliqués ; le dépasser entraîne un refus explicite. Ce ne sont pas des quotas de disque ni une garantie pour un historique arbitrairement volumineux.
- Les gros diffs affichent chemins, identités, tailles et modes sans charger tout le texte ; le rendu détaillé est conservé pour les comparaisons totalisant au plus 8 MiB. Les API historiques retournant tous les octets et la résolution sémantique gardent leurs propres limites de 64/256 MiB.
- Lens WASM isolée et reçus testés sous Windows et Linux x86_64 ; distribution signée, recette macOS et pilote PME externe restent à effectuer.
- Certaines anciennes surfaces/code sont encore présents pendant le raccordement natif. Le pilote Compose HTTPS local est documenté dans `deploy/selfhost/LOCAL-PILOT.md`. La recette visuelle Web reste distincte de la recette Desktop native.

## Prévisualiser une restauration et rétablir un marker

`layrs restore @marker:baseline --path fichier.txt --preview --json` décrit les
changements et collisions sans capturer le travail présent. Pour appliquer ce
plan, passez l'objet `data.preview.expected` comme JSON à `--expected` avec la
même cible et le même chemin. Un changement d'Operation, de contenu ou de portée
impose une nouvelle prévisualisation. `restore-view OPERATION_ID` accepte les mêmes
options. Desktop transmet ce contrôle automatiquement après son aperçu.

`layrs marker show MARKER_ID` rétablit la visibilité de l'objet immuable masqué,
sans déplacer les Layers ni capturer les fichiers présents. Desktop propose
« Rétablir un marker masqué » avec son identifiant exact. Les noms de markers
peuvent être ambigus : dans ce cas, utilisez l'identifiant. Une Release corrigée
utilise un nouvel objet et un nouveau numéro de version ; la révocation conserve
l'ancien manifeste.

## Sélection Git

```text
layrs git import DEPOT_GIT --ref refs/heads/main --ref refs/tags/v1.0 --dry-run
layrs git import DEPOT_GIT --ref refs/heads/main --ref refs/tags/v1.0
layrs git export DESTINATION_NEUVE --ref refs/heads/main
```

Les références doivent être complètes et exactes. Chaque sélection conserve tous
les ancêtres nécessaires, y compris les parents de fusion. Les noms d'export
correspondent aux Layers et points natifs ; un Layer à plusieurs têtes exige de
choisir sa référence de tête explicite. La reprise d'export exige le même ensemble
de références, quel que soit leur ordre. `--snapshot` est incompatible avec `--ref`.
L'import rejoué valide sa provenance et son reçu natif. Pour conserver une
acquisition complète avant conversion, créer un cache hors du dépôt source :

```text
layrs git scan-cache DEPOT_GIT --output CACHE --ref refs/heads/main --json
layrs git import-cache CACHE --digest sha256:DIGEST --dry-run --json
layrs git import-cache CACHE --digest sha256:DIGEST --json
```

Conserver séparément le digest complet renvoyé par la première commande. La
reprise vérifie le cache et reconstruit le plan sans source ni processus Git.
Elle recommence la conversion depuis le premier commit du cache ; une acquisition
interrompue avant sa publication doit être recommencée. Le cache contient les
octets sensibles de l'histoire sélectionnée : protéger son emplacement et son
digest, sans le ranger dans les métadonnées `.layrs`. Les modifications de source
postérieures au cache ne sont pas incluses. Détails et bornes :
[pont Git](../crates/layrs-git-bridge/README.md).

## Revue, preuves et invitations depuis la CLI

L'historique publié est consultable avec `layrs remote history STEP_ID --endpoint
https://layrs.example.com`, puis `layrs remote history STEP_ID CURSOR --endpoint
https://layrs.example.com` pour la valeur `next` reçue. La page contient les captures
et leurs parents, ancrés sur le même Step immuable même si une Layer avance.
Desktop et Web proposent **Historique publié** et une comparaison avec chaque
parent. Les droits sont revérifiés à chaque page ; après changement de droits,
recommencer le parcours. Les observations locales (date, acteur et motif de
capture automatique) ne sont pas inventées sur le serveur. Les filtres locaux
par chemin/acteur restent dans **Explorer l'historique** du Desktop.

Bornes du parcours distant : 1 à 100 captures par page (30 dans la CLI/UI),
100 000 objets visités, 64 Mio de métadonnées lues et 2 Mio de captures retournées.
Une capture inaccessible ou corrompue fait échouer la page entière ; aucun trou
silencieux n'est présenté comme un historique complet.

```text
layrs remote reviews --endpoint https://layrs.example.com
layrs remote review REVIEW_ID --endpoint https://layrs.example.com
layrs remote review-action REVIEW_ID commentaire.json --endpoint https://layrs.example.com
layrs remote runs --endpoint https://layrs.example.com
layrs remote run RUN_ID --endpoint https://layrs.example.com
layrs remote run-cancel RUN_ID annulation.json --endpoint https://layrs.example.com
layrs remote invitations-inbox --endpoint https://layrs.example.com
layrs remote invite-member invitation.json --endpoint https://layrs.example.com
layrs remote invitation-decision INVITATION_ID accept --endpoint https://layrs.example.com
layrs remote storage-budget --endpoint https://layrs.example.com
```

`reviews` et `runs` renvoient `nextCursor` : passez-le comme argument pour continuer.
Un changement de droits exige de recommencer la lecture. Les fichiers de requête
JSON sont limités à 256 Kio ; ils contiennent les données métier, jamais le token
de session. Exemple de commentaire :

```json
{"idempotencyKey":"commentaire-unique-0001","version":1,"action":"comment","body":"À vérifier","anchor":null,"replyTo":null}
```

Pour annuler une exécution : `{"idempotencyKey":"annulation-unique-0001","reason":"Remplacée par une nouvelle capture"}`.
L'annulation bloque toute preuve ultérieure et conserve une trace immuable. Elle
ne termine pas un conteneur externe à distance ; le délai du runner reste applicable.

Le runner enregistre d'abord sa préparation (`POST /runs/pending`) : l'écran
indique **En attente de démarrage** pendant le téléchargement et la vérification
du Step exact. Après matérialisation, son signal `POST /runs/RUN_ID/begin` fait
passer l'état à **En cours**. Le serveur refuse une preuve avant ce signal et
refuse le démarrage après annulation, expiration ou changement des autorisations.
Le signal est une déclaration de l'exécuteur, pas une surveillance de son processus.
Les intégrations utilisant l'ancien `POST /runs` déclarent toujours directement
leur exécution en cours. Aucun ordonnanceur hébergé n'est fourni.

Une invitation indique `recipientPrincipalId`, `role` (`member` ou `auditor`),
`expiresInSeconds` et `idempotencyKey`. Le destinataire doit déjà disposer d'un
compte sur l'instance et accepter explicitement. Aucun email n'est envoyé.
Voir [le contrat des invitations](../deploy/selfhost/INVITATIONS.md).

Les **Équipes du projet** regroupent des comptes humains déjà actifs dans
l'organisation. Chaque équipe accorde un rôle (`member`, `auditor` ou
`release_manager`) dans le projet courant. Retirer un membre ou archiver une
équipe retire ses droits propres ; les accès individuels et ceux d'autres équipes
restent indépendants, et une interdiction explicite reste prioritaire. La gestion
requiert l'administration du projet. Le serveur conserve chaque version pour audit.

```text
layrs remote teams --endpoint https://layrs.example.com
layrs remote team-members --endpoint https://layrs.example.com
layrs remote team-create equipe.json --endpoint https://layrs.example.com
layrs remote team-update TEAM_ID equipe.json --endpoint https://layrs.example.com
layrs remote team-history TEAM_ID --endpoint https://layrs.example.com
layrs remote team-archive TEAM_ID archivage.json --endpoint https://layrs.example.com
```

Création : `{"idempotencyKey":"equipe-design-0001","expectedVersion":0,"name":"Design","role":"member","memberPrincipalIds":["PRINCIPAL_ID"]}`.
Pour modifier, utilisez la version courante dans `expectedVersion` et une nouvelle
clé. Rejouez exactement le même fichier et la même clé si la réponse est perdue.
Une version périmée est refusée : relisez l'équipe avant de décider du changement.
L'archivage exige `idempotencyKey`, `expectedVersion` et `reason`. La pagination
d'historique reprend avec `team-history TEAM_ID NEXT_AFTER ANCHOR_VERSION`.
Bornes actuelles : 32 équipes conservées (archives comprises), 64 membres par équipe et budget partagé des
32 entrées de politiques. La création d'un compte ou son admission dans
l'organisation reste une action distincte.

Pour choisir explicitement les têtes distantes, l'administrateur prépare un
fichier `choix.json` : `{"layer":"LAYER_ID","chosenHeads":["STEP_ID"],"reason":"Motif de la décision"}`.
Un tableau vide retire toutes les têtes du Layer sans supprimer ses anciennes
captures. Les protections et preuves exigées restent appliquées aux têtes choisies.

```text
layrs --json remote choose-preview choix.json --output apercu.json --endpoint https://layrs.example.com
layrs remote choose-apply apercu.json --endpoint https://layrs.example.com
```

L'aperçu liste têtes actuelles, nouvelles têtes et têtes écartées, et capture la
View/génération attendues. `--output` crée un fichier UTF-8 neuf, sans écraser
d'aperçu existant. Après une réponse perdue, rejouez **le même aperçu**. Une vue
périmée exige un nouvel aperçu explicite. Desktop expose ce parcours dans
**Choisir les têtes distantes · administration**. Seuls les administrateurs de
l'organisation possédant aussi `policy.administer` peuvent appliquer ce choix.
Il ne modifie pas le checkout local ; utilisez ensuite `remote receive`.

Les brouillons Web/Desktop restent sur l'appareil, isolés par serveur, compte,
projet, revue et version. Leur restauration est explicite ; aucun envoi automatique.
Une notification ouvre la version concernée, en lecture seule si elle est ancienne.
Une correction de commentaire crée une révision auditée et conserve le texte initial,
son auteur, sa version et son ancrage. Seul l'auteur encore autorisé peut la publier :

```text
layrs remote review-correct REVIEW_ID COMMENT_ID correction.json --endpoint https://layrs.example.com
layrs remote metrics --endpoint https://layrs.example.com
```

Exemple de correction : `{"idempotencyKey":"correction-unique-0001","version":1,"expectedRevision":0,"body":"Texte précisé","reason":"Correction d'une imprécision"}`.
`expectedRevision` vaut zéro pour le texte original, puis la dernière révision lue.
Une révision concurrente impose de recharger ; rejouer exactement la même requête
retrouve la même trace. Les métriques sont réservées à l'administrateur de l'instance.

## Lire et comparer les variantes d'un conflit

```text
layrs weave conflicts
layrs conflict-variant CONFLICT_ID TERM_ID --layer main
layrs conflict-variant CONFLICT_ID TERM_ID --layer main --against BASE_OR_OTHER_TERM_ID
```

La comparaison est en lecture seule, avec IDs et diff textuel borné. Les contenus
binaires, structurels ou dépassant les limites restent identifiés explicitement.
Le Desktop propose la même comparaison entre bases et variantes avant le choix.
Une résolution n'est enregistrée qu'à l'application de son aperçu.

Les conflits natifs sont des Steps immuables. `resolve-term` ou `resolve-file`
enregistrent la résolution ; la dernière résolution installe les fichiers.
`continue` n'est pas nécessaire. Pour revenir à une View antérieure, utiliser
`restore-view OPERATION --preview`, puis l'application protégée, ou `undo`.
Ces actions ajoutent de l'histoire et conservent le Step conflictuel.

Un conflit de renommage peut concerner plusieurs chemins. `resolve-term` choisit
alors une variante complète pour tous ces chemins ; Desktop présente leur liste
et le même aperçu. La comparaison d'un fichier seul et `resolve-file` refusent ce
cas, car ils ne décriraient pas la création/suppression des autres chemins.

## Historique distant et transformations publiées

```text
layrs remote history STEP_ID --path docs/readme.txt --endpoint https://layrs.example.com
layrs remote history STEP_ID --actor actor:IDENTIFIANT --since 2026-09-01T00:00:00Z --endpoint https://layrs.example.com
layrs remote publish-candidates --endpoint https://layrs.example.com
layrs remote choose-preview choix.json --output apercu.json --endpoint https://layrs.example.com
layrs remote choose-apply apercu.json --endpoint https://layrs.example.com
```

Le filtre distant accepte un chemin exact de 256 octets UTF-8 maximum. Réutiliser
le curseur exige le même chemin. Il compare chaque capture à ses parents et ne
déduit pas une continuité de renommage. L'interface Desktop/Web propose ce filtre.

`--actor`, `--since` et `--until` filtrent la première réception authentifiée sur le
serveur, avec bornes de date inclusives. L'acteur ayant publié et la date de
réception sont affichés séparément du Step : ils n'attribuent pas la création de
son contenu. Les captures admises avant cette fonctionnalité restent sans cette
métadonnée. Le curseur conserve les filtres et le snapshot des observations ; un
ancien curseur v1 exige de recommencer la recherche.

Après un regroupement de captures déjà publiées, capturez le travail courant avec
`layrs step`. `publish-candidates` admet sa fermeture immuable sans déplacer les têtes ni les
points nommés distants. Le choix administratif est une seconde action explicite,
avec aperçu et protections habituelles. Un ancien clone conserve ses têtes
concurrentes après réception ; aucun effacement de son histoire n'est implicite.
Une publication refusée produit une erreur CLI/Desktop ; elle n'est pas annoncée
comme réussie. Après une réponse perdue, rejouer la même action retrouve son reçu.

Desktop permet d'annuler clones, réceptions et publications ; la CLI utilise
Ctrl+C pour demander la même annulation. Réception et publication exigent un
checkout propre, sans capture automatique implicite. L'annulation attend la
réponse de la requête en cours (timeout 30 s) ;
si l'application a déjà commencé, l'interface l'indique et attend son résultat.
Après annulation confirmée, les fichiers et l'historique local sont conservés.
Une publication annulée avant sa transaction ne déplace pas les têtes distantes ;
des objets transférés peuvent rester orphelins. Dès qu'une transaction est
journalisée ou a pu être envoyée, l'annulation n'est plus confirmable : attendez
le reçu ou rejouez la publication exacte après une réponse perdue. Fermer
brutalement le processus n'est pas une confirmation d'annulation.
