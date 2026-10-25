# Worktrees parallèles, disponibilité partielle et VFS

Programme approuvé le 24 octobre 2026. Base :
`4601b1694a4737183049e3c12124141b24860b92` (`redesign/foundations`).
Branche de réalisation : `feature/worktrees-partial-vfs`.

## Résultat attendu

Deux agents travaillent simultanément sur une même machine dans deux Worktrees
isolés, éventuellement virtuels et partiels. Ils partagent les contenus immuables
autorisés, gardent leurs propres modifications et captures, puis intègrent
explicitement leurs résultats. Les données préparées restent utilisables sans
serveur. Les gains de performance sont mesurés sur les parcours réels.

L'approbation couvre le programme et les décisions techniques. Elle ne vaut pas
autorisation de commit, push, déploiement, publication ou conversion des données
réelles d'un utilisateur. Les essais de conversion utilisent des copies jetables.

## Lots et suivi

| Lot | Contenu | Dépendances | État |
| --- | --- | --- | --- |
| L0 | Contrats, instrumentation et baseline reproductible | — | En cours |
| L1 | Worktrees multiples sur Store complet, locator et opérations indépendantes | L0 | En cours |
| L2 | Sélection volontaire de chemins, clone partiel et élargissement | L0 | En cours |
| L3 | Couverture hors ligne, pins, quotas et Store partiel partagé versionné | L2 | Contrat revu, backend en cours |
| L4 | Prototype Windows VFS sur CAS local complet | L1 | Fournisseur natif testé, intégration CAS en cours |
| L5 | Convergence VFS/partiel, récupération et qualification des plateformes | L3, L4 | Primitives de contrôle testées ; intégration partielle et reprise gérée en cours |
| L6 | Comparaison, transferts et accélérateurs justifiés par les mesures | L0 | Tests ciblés passés, corrections de revue en cours |
| L7 | API/CI ciblées, intégration exacte et fraîcheur des approbations | L0 | Deux recettes serveur complètes passées, revue indépendante R2 PASS |
| L8 | CLI/Desktop, parcours agents, disponibilité et pont Git guidé | L1, L2, puis L3/L5 | Surfaces écrites, validation native en attente |
| L9 | Recettes intégrées, incidents, documentation et revues indépendantes | Lots concernés | Revues et tests en cours |

Un lot n'est terminé que lorsque ses critères ci-dessous ont une preuve datée.
Le premier jalon L1/L4 ne clôt pas le programme.

## Contrats communs

1. Les identités canoniques Step, Tree et FileId ne changent pas. Le CAS contient
   les octets immuables ; les métadonnées ne publient une capture qu'après leur
   installation durable et vérifiée.
2. Chaque Worktree possède un identifiant indépendant, une racine validée et une
   Operation active. Une View globale ne remplace pas la View propre d'un autre
   Worktree. Les attentes CAS empêchent les écrasements concurrents.
3. La base d'une tâche est une Step exacte et stable. Une réception ou un changement
   de Layer n'altère pas implicitement les fichiers utilisés par l'autre agent.
4. Les fichiers modifiables, fichiers temporaires, sorties de build et journaux
   sont privés. Aucun hardlink modifiable ne relie un Worktree au CAS ou à un autre
   Worktree. Le VFS n'est pas une sandbox de sécurité.
5. Sélection utilisateur, droits observés, présence vérifiée, épinglage et travail
   local sont distincts. Exclusion et non-résidence ne signifient pas suppression.
6. Une couverture hors ligne nomme chemins, versions et opérations disponibles.
   Un manque est signalé sans faux fichier vide ni mutation partielle présentée
   comme un succès. Les modèles IA ou services externes ne deviennent pas offline.
7. Les captures acceptées, pins, bases nécessaires et journaux sont protégés de
   l'éviction. Fermer/démonter un Worktree ne supprime pas son travail non intégré.
8. Un cache n'est jamais une autorisation. Les acquisitions et publications
   revalident les droits courants. Une révocation ne prétend pas effacer des
   octets déjà obtenus ; les changements locaux restent récupérables.
9. Le Store complet conserve sa fermeture complète. Le Store partiel évolue dans
   une famille de format explicitement reconnue ; les anciens lecteurs doivent
   refuser la mutation d'un format incompatible.
10. Événements FS et stat sont des accélérateurs. Le mtime seul n'établit jamais
    l'absence de modification. Les contrôles critiques précèdent tout écrasement.

## Critères de sortie par lot

### L0 — Contrats et mesures

- Documenter les interfaces et les scénarios avant les changements persistants.
- Baseline : création, premier fichier utilisable, capture, diff, réception,
  historique, publication, attente de verrou, CPU/mémoire/disque/octet réseau.
- Profils code, assets peu compressibles et mixtes, caches froids/chauds, un/deux
  agents ; 1k/10k/99k fichiers dans les plafonds existants.
- Conserver machine, versions, fixtures, commandes et résultats bruts. Calibrer
  les budgets sur la dispersion ; aucun multiplicateur annoncé sans mesures.

### L1 — Worktrees natifs

- Création depuis une Step/Layer exacte dans une racine vide validée, locator
  explicite, ouverture, découverte et liste ; compatibilité du checkout primaire.
- Utiliser les journaux et leases de matérialisation. Une création interrompue
  reste récupérable et n'est pas annoncée prête.
- Deux captures concurrentes gardent leurs Operations et variantes. Changer la
  Layer d'un Worktree ne déplace pas la base de l'autre.
- Tester identités remplacées, liens, locator incohérent, réouverture, fsck et GC.

### L2 — Sélection partielle

- Sélection normalisée de chemins exacts/sous-arbres, par génération locale.
- Le clone ne télécharge que les contenus sélectionnés et autorisés ; les
  métadonnées de réservation restent bornées et peuvent encore être complètes.
- Élargissement explicite avec contrôles de collision/base/génération/autorité.
- Capture, réception et rebase préservent les chemins exclus. Une désélection ne
  produit aucun Tombstone ; le retrait/éviction attend L3.

### L3 — Disponibilité et stockage partagé

- Préparation hors ligne atomiquement annoncée après installation vérifiée.
- Lecture, édition, capture, comparaison et restauration des états préparés sans
  réseau ; manque précis si une base/version requise n'a pas été préparée.
- Pins, quotas et retrait propre sans perte ; refus initial de dématérialiser du
  travail modifié ou conflictuel.
- Évolution versionnée du Store partiel : générations/captures/journaux privés,
  contenus immuables partagés dans un même contexte d'identité. Revue du schéma et
  de la conversion avant implémentation ; source conservée et essais de panne.

### L4/L5 — VFS

- Commencer par deux racines Windows servies depuis un Store complet local.
- ProjFS est le candidat initial ; CFAPI est évalué si les prérequis ou usages le
  justifient. Aucun pilote noyau Layrs personnalisé.
- Un fournisseur expose les données immuables et garde les écritures privées.
  Status/capture ne doivent pas hydrater tous les fichiers pour les scanner.
- Qualifier rename, suppression, handles ouverts, mmap, watchers, flush, crash,
  disque plein et redémarrage ; distinguer réseau absent et service arrêté.
- Brancher ensuite le même moteur de disponibilité partielle et prévoir un export
  de récupération vers le mode ordinaire. Linux/FUSE suit sa propre recette ;
  macOS et volumes non qualifiés ne sont pas annoncés supportés.

### L6 — Performance

- Comparaison : éviter les lectures de contenus identiques, tout en conservant
  autorisations par chemin, contrôles de type/conflit et budgets logiques.
- Transferts : réemploi d'objets admis vérifiés, plans groupés et concurrence
  bornée avec reprise, annulation et autorisation exacte.
- Capture : exploitation prudente du journal/événements et fallback exact.
- Index dérivés reconstruisibles, modification de verrou, chunking ou cache
  supplémentaire seulement après preuve d'un coût. Le verrou CAS protège aussi
  l'ordre des verrous et le pool : ne pas le retirer par simple inspection.

### L7/L8 — Parcours et intégration

- API de consultation/candidats/patchs ancrées, bornées et autorisées ; export CI
  d'une Step exacte et de toutes les entrées requises par sa recette.
- Intégration A/B explicite, conflits conservés, preuves sur le résultat combiné.
- Tester l'interférence entre Layers/Markers sur la validité des approbations
  avant de décider d'une éventuelle portée plus fine ; conserver exactStep.
- CLI et Desktop réutilisent le même cœur ; état de Worktree, sélection,
  disponibilité, agent/tâche et récupération visibles et accessibles.
- Pont Git Desktop guidé sur le moteur existant ; ne pas ajouter implicitement
  LFS, submodules ou export continu.

### L9 — Validation intégrée

- Deux processus changent le même chemin différemment, capturent, coupent le
  réseau, redémarrent après panne et retrouvent leurs deux variantes.
- Vérifier intégration sans conflit puis conflit, rétention sous pression disque,
  collisions/ACL, migration interrompue et refus des lecteurs incompatibles.
- Exécuter les contrôles pertinents Rust/TS/CLI/Desktop/Web/PostgreSQL et les
  recettes natives VFS ; les tests UI seuls ne prouvent pas la sûreté du FS.
- Revue indépendante de spécification et revue technique critique pour
  concurrence, persistance et compatibilité, puis résultat intégré.
- Runbooks de sauvegarde/récupération et rapport séparant passé/échoué/non exécuté.
  Opérateur indépendant, pilote humain et plateforme absente restent des limites
  tant qu'ils n'ont pas réellement été qualifiés.

## Répartition actuelle

- Coordination : contrats, environnement, CLI/Desktop, interfaces et intégration.
- Agent L1 : workflow natif Worktrees, API Store nécessaire et tests associés.
- Agent L2 : modèle/projection, sélection persistée, clone/receive/rebase et tests.
- Agent L6A : comparaison serveur et tests différentiels/comptage.
- Étude L4 : adaptateur Windows et prérequis, en lecture seule.

Les fichiers partagés ont un propriétaire. Les auteurs ne vérifient pas seuls
leurs changements. Pas de compilation globale ou formatage concurrent mutatif.

## Journal des décisions et preuves

### 2026-10-24 — Préparation

- Nouveau worktree T3 vérifié sur la branche et le SHA attendus. `.repowise/`
  apparaît déjà non suivi ; préservé. Aucun index RepoWise disponible.
- Node `v24.19.0`, pnpm `9.15.0`, Docker `27.2.0` disponibles. Les outils C++
  Visual Studio 2019 et Windows SDK sont présents ; Rust absent initialement.
- Installation Rust 1.85.0 demandée conformément à `rust-toolchain.toml`, sans
  modification du PATH utilisateur ; dépendances JS via lockfile gelé en cours.
- L1 : supprimer les hypothèses de tête globale unique dans les workflows,
  préserver les têtes d'Operations étrangères sous CAS exact.
- L2 : sélection locale indépendante, versionnée et liée aux générations ;
  upstream projection/identités canoniques inchangés si le contrat le permet.
- L6A : les métadonnées actuelles n'autorisent pas un saut arbitraire de sous-arbre
  sans contrôles descendants. Réduire d'abord les lectures de contenus tout en
  conservant parcours d'autorisation et budgets ; gain à mesurer.

### 2026-10-24 — Première intégration

- Rust 1.85.0 installé et utilisé avec les outils MSVC existants ; installation
  JS `pnpm install --frozen-lockfile` réussie. Aucun changement du PATH permanent.
- Référence originale : compilation CLI/client réussie, tests CLI de sécurité
  locale 18/18, maintenance 1/1, partiel 5/5, renommage 1/1. Le workflow CLI a
  d'abord échoué sur deux tests faute de `layrs-lens-worker`, puis 13/13 après
  construction de ce prérequis existant. Cela ne valide pas les changements.
- L6A avant optimisation : 9 tests différentiels/limites réussis, commande
  `cargo test --locked --target-dir .layrs-local/target -p layrs-store-server v3::compare --lib -- --nocapture`.
- Premier test du Store partiel après reconstruction : 85 réussis, 2 échecs
  `DatabaseBusy` dans les captures/restaurations volumineuses ; correction en
  cours. Journal brut `.layrs-local/validation/l2-store.log` (ignoré par Git).
- Un artefact `layrs-core` périmé issu de la référence partageait le répertoire
  de compilation. Nettoyage ciblé du paquet compilé, puis reconstruction ; aucun
  changement de source pour masquer cet échec d'environnement.
- L1/L2 et surfaces CLI/Desktop écrites ; tests Rust intégrés et revues de
  spécification/concurrence/migration en cours. Les lots ne sont pas terminés.
- ProjFS absent sur cette machine (DLL/service/pilote introuvables) ; CFAPI est
  présent. Prototype Windows CFAPI dans `layrs-vfs`, chargé dynamiquement, avec
  deux racines privées et lecture locale. Montage natif et intégration à la
  capture restent à vérifier. Aucun composant Windows activé ou installé.
- Transfert : conserver la requête finale revalidée pour chaque objet réutilisé.
  Le regroupement des plans ne dispense jamais d'autoriser chaque chemin exact.
  Pas de gain global de requêtes annoncé face à l'ancien parcours qui omettait
  certains contrôles par chemin.
- Ajouts de dépendances limités aux dépendances de test TLS déjà verrouillées
  (`rcgen`/`rustls`) et au fournisseur Windows `windows-sys` déjà verrouillé ;
  aucune version tierce modifiée dans le lockfile.

### 2026-10-24 — Corrections et qualification intermédiaire

- L2 : les 87 tests du Store partiel passent après correction de la lecture de
  sélection par une seconde connexion SQLite pendant une transaction d'écriture.
  Preuve : `.layrs-local/validation/l2-store-fixed.log`. Cette preuve précède L3.
- L6A : 14 tests passent, avec réemploi vérifié et contrôles par chemin conservés.
  Dans la fixture, lectures Tree 10→4 et manifestes/chunks 6→1. Pas de mesure de
  latence de production. Preuve : `l6a-compare.log` dans le même répertoire.
- L6B : première version, 16 tests passent. Pour les mêmes claims autorisés,
  plans 11→6, GET 18→18, octets d'objets 201423→1425. La revue a demandé une
  correction de division des lots dépassant le budget agrégé et un test réel de
  renouvellement HTTPS ; ces corrections attendent leur nouvelle exécution.
- L1 : 17 tests du contrat Store, puis 5 tests d'admission locale, 11 tests de
  capture et 3 de concurrence passent (`l1-store-all.log`,
  `l1-parents-fixed.log`, `l1-integrated-fixed.log`). La suite intégrée s'est
  ensuite arrêtée sur une fixture VFS supposant des blocs de 64 Kio au lieu des
  blocs de capture disque de 1 Mio. Les suites suivantes restent à exécuter.
- Revue Opus 5.5, deuxième passe : la provenance du pointeur actif doit aussi
  être contrôlée lorsque le parent appartient encore à la frontière courante.
  Correction et tests supplémentaires en cours. Aucune acceptation L1 définitive.
- CFAPI utilise désormais PROGRESSIVE. Les 9 tests unitaires et le test natif
  à deux racines passent. La revue critique a démontré que les handles de seuls
  attributs n'excluaient pas les écritures/suppressions ; des handles participant
  réellement au partage ont été ajoutés. Nouvelle preuve native réussie avec
  refus des accès concurrents et inspection sans hydratation :
  `l4-p1-guards-unit.log` et `l4-p1-guards-native.log`.
- Le noyau demande toujours les fichiers entiers : 8388682 octets transférés
  pour 142 octets lus par les applications. Aucun gain de résidence par page
  annoncé. Validation des tailles des callbacks et intégration CAS/capture/hôte
  toujours en cours ; le montage seul ne réalise pas L4/L5.
- Desktop : 35 tests de modèle, 24 tests navigateur sur fixtures, types et build
  passent (`desktop-surfaces.md` et logs associés). La revue du pont Git impose
  des corrections des identités de destination/racine, du contexte d'autorité et
  de la publication depuis des Worktrees indépendants. Tests natifs à exécuter.
- L3 : contrat de famille partielle format 4 revu, avec revendications privées
  durables, provenance importée conservée, quota de capture effective et lecteurs
  protégés. Implémentation en cours ; aucune conversion de données réelles.
- PostgreSQL de qualification lancé dans un conteneur jetable explicitement
  dédié à ce programme ; recettes serveur encore à exécuter. Les journaux bruts
  locaux restent ignorés par Git. Aucun commit, push ou déploiement.

### 2026-10-24 — Qualification des frontières et reprises

- L6B : nouvelle exécution après les corrections de division des lots et de
  renouvellement HTTPS, **19/19** tests réussis (`l6b-download-r2.log`). Une
  nouvelle revue indépendante porte sur ces corrections et leurs consommateurs.
- Fournisseur Windows : **13/13** tests unitaires et **1/1** recette native
  réussis (`l4-provider-api-unit.log`, `l4-provider-api-native.log`). Adaptateur
  CAS : **6/6** tests réussis (`l4-source-runtime.log`). Ces résultats ne valident
  pas encore la capture complète, le journal ni l'hôte VFS.
- Les trois premières recettes VFS intégrées ont échoué avec un refus d'accès.
  Le diagnostic natif a isolé l'absence de `READ_CONTROL` avant la lecture des
  ACL d'un répertoire privé. Le test de réouverture privée passe après correction
  (`l1-private-open-r3.log`). Les recettes complètes doivent être réexécutées.
- Worktrees : les premières suites ont donné 12 réussites/1 échec de création
  concurrente, 10/1 en sécurité et 1/1 dans la suite avec sous-processus. Les
  corrections portent sur les lectures en contention et les assertions de
  reprise, sans assouplir le CAS. La fixture Windows multi-volume est indisponible
  et reste une qualification sautée, même si le harnais termine son test.
- Git : **6/6** tests des destinations ancrées passent (`l8-anchored-export.log`).
  Suite du pont, tests Tauri et nouvelle revue Opus 5.5 restent nécessaires.
- L3 : **13/13** tests partagés ont passé avant les dernières corrections de
  récupération et de quotas (`l3-shared-r2.log`). La revue Opus a identifié des
  défauts de réservation, d'éviction et de conversion ; leurs corrections
  attendent une nouvelle exécution. Les premiers tests de l'adaptateur réseau
  ont échoué dans l'initialisation des fixtures, corrigée depuis sans modifier
  le protocole ; aucun succès de préparation réseau n'est encore revendiqué.
- La revue VFS intégrée impose aussi une publication récupérable du contrôle
  `.layrs`, une résolution de reçu tolérant la contention transitoire et un délai
  absolu pour les trames locales non authentifiées. Ces travaux restent ouverts.
- L5 utilise une famille partielle et un adaptateur distinct, jamais un Store
  SCM complet avec des objets absents. La conception détaillée et ses critères
  de sortie sont consignés séparément ; sa seule rédaction ne termine pas L5.

### 2026-10-24 — Tests natifs, serveur réel et corrections de parcours

- Le parcours HTTPS/PostgreSQL de sélection et de contrôle d'accès réussit
  (`l2-live-tls-selection-r2.log`, 249,99 s). Il couvre notamment clone sélectionné,
  élargissement, publication, réception, révocation et preuves du candidat ciblé.
  La première invocation avait échoué avant le scénario, faute du conteneur
  PostgreSQL jetable attendu. La recette CI complète avec Docker reste distincte.
- Préparation hors ligne côté client : **13/13** tests réussis, dont acquisition
  d'un chemin inchangé d'une capture depuis sa génération exacte, provenance
  importée, et refus de réparation distante d'un remplacement capturé corrompu
  (`l3-offline-capture-r3.log`). La nouvelle correction des quotas et journaux du
  backend L3 attend encore sa qualification complète.
- VFS : **15/15** tests unitaires et deux recettes fournisseur natives réussissent
  après correction des chemins longs. La reprise conserve l'identité native et
  les octets du contrôle sur un chemin de 438 caractères, sans hydratation
  (`l4-provider-longpath-{unit,native,control}.log`). Les quatre recettes intégrées
  ont réussi dans `l4-complete-{worktrees,recovery,contention}-r3.log` et
  `l4-complete-creation-longpath.log`. Ces preuves ne qualifient ni L5, ni la
  résistance aux coupures électriques, ni un gain de résidence par page.
- Les alias secondaires Windows passent la suite de sécurité
  (`l1-safety-alias-r4.log` : 13 scénarios exercés, fixture multi-volume absente).
  La revue Opus n'a trouvé aucun P0/P1/P2 dans cette correction. Une protection
  supplémentaire de racine remplacée et des tests de locator sont ajoutés ; leur
  réexécution reste nécessaire. Le test Windows multi-volume est désormais
  explicitement ignoré en exécution ordinaire.
- Les trois tests natifs de découverte/récupération et d'inspection de conversion
  passent (`l8-recovery-routing-native.log`) : le journal format 4 reste intact
  jusqu'à la reprise explicite ; l'inspection d'une source schema 5 refuse sans
  modifier ses octets ni sa version ; une copie avec identité incohérente refuse.
- Desktop : 53 tests de modèle et 50 tests navigateur sur fixtures, types et
  build passent après correction du chargement automatique de statut sur la page
  de conversion. Les nouveaux parcours de restauration locale attendent leurs
  propres tests. Tauri/WebView réel reste à qualifier.
- Le pont Git a passé 33 tests avant ses dernières corrections. Le nouveau
  contrôle ancré a révélé un `sync_all` sur handle de vérification en lecture
  seule ; correction en cours. La revue Opus demande aussi la normalisation des
  locators Git relatifs sous Windows et une seule liaison source par acquisition.
  Aucun résultat antérieur ne valide ces dernières corrections.

### 2026-10-24 — Sérialisation de création et récupération explicite

- L1 : les six tests de création concurrente passent après correction du délai
  du verrou de création (`l1-creation-r5-fixed.log`). Les suites suivantes passent
  aussi (`l1-runtime-r5.log`) : 3 tests de concurrence, 16 tests de sécurité,
  13 résultats Worktrees dont un helper, et 2 résultats processus dont un helper.
  Le second volume Windows reste explicitement ignoré. Les 7 tests de parents
  passent (`l1-parents-r5.log`). La revue Opus R5 ne relève aucun P0/P1/P2 dans
  les verrous et reprises ; les délais restent bornés et ne garantissent pas
  qu'une création longue sera terminée avant l'expiration d'un autre appel.
- L3 : 36 tests partagés et la régression complète de 123 tests passent
  (`l3-shared-r4-fixed.log`, `l3-projection-r4.log`). Ces résultats précèdent la
  nouvelle liaison du destinataire de restauration, la découverte typée des
  journaux SQLite et la limitation de l'historique des préparations libérées.
  Cette nouvelle révision a ses tests et une revue indépendante en cours ; les
  résultats précédents ne lui sont pas attribués.
- Fournisseur VFS : 18 tests unitaires et les deux recettes natives passent
  (`l4-provider-r4-fixed-{lib,native,control}.log`). Le premier essai R4 avait
  révélé que `.layrs` étranger était accepté comme fichier ordinaire ; le refus
  des noms de métadonnées a été corrigé sans assouplir l'exclusion par capacité.
  La reprise des cinq interruptions et du chemin long conserve les identités.
  Les nouveaux cas intégrés de création sur une même racine, de journal plein
  et de catalogue à la limite attendent leur exécution.
- Git : 9 tests d'export ancré et 40 tests du pont passent, avec 2 tests de
  mesure ignorés (`l8-anchored-export-r3.log`, `l8-git-bridge-r3.log`). La revue
  Opus R4 ne relève aucun P0/P1/P2. Les tests natifs Tauri restent à exécuter.
  Les ajustements P3 de tampon d'écriture et de locator contenant `..` devront
  recevoir leur propre validation.
- Desktop : les auteurs ont fourni 60 tests de modèle, 65 tests navigateur sur
  fixtures, types et build réussis (`desktop-typed-recovery-handoff.md`). Les
  preuves natives de récupération après arrêt SQLite, de restauration depuis
  une observation distante et les parcours CLI restent séparées de ces fixtures.
- L5 reste proposé et non implémenté. Les mesures avec les vrais binaires,
  l'auditeur commun et les recettes CI complètes restent nécessaires ; aucune
  supériorité de performance ni acceptation globale n'est annoncée.

## Qualification liée et lancement séquencé de L5 — 24 octobre

- La récupération liée et Restore passent leur revue SPEC R5 ciblée sans
  P0/P1/P2 restant. Les journaux frais sont `l3-projection-r6.log` (140),
  `l3-bound-recovery-r6.log` (6), `l3-schema5-discriminating-r6.log` (1),
  `l8-recovery-binding-r6.log` (10 et un helper ignoré),
  `l8-cli-offline-r6.log` (17), `l8-cli-partial-r6-fixed.log` (30 et 9),
  et `l2-offline-r6.log` (14). Le test schema-5 révisé utilise le schéma réel
  et un contrôle positif de migration ordinaire ; il complète les 140 tests.
  Le WebView réel et les commandes CLI explicites de récupération SQL/init
  restent hors de cette clôture ciblée.
- Le VFS R5 a 18 tests fournisseur, 7 tests de journal, 10 tests client et
  8 tests de source réussis. Les recettes natives de contrôle, remplacement
  de casse, avance externe répétée et neuf interruptions d'activation passent.
  Les deux tests client natifs ignorés et les autres recettes intégrées restent
  dans la file. Les journaux `l4-*-r5*.log` conservent ces résultats et les deux
  erreurs initiales de fixture SQL, corrigées sans toucher à la production.
- La sonde native confirme douze renommages de placeholders après déconnexion
  ou arrêt du processus. La revue VFS R6 retient le blocage de l'export comme
  P2. Une capacité distincte réservée à l'export est autorisée : nom courant
  exact, fichier réel retenu, hash intégral et refus précis des octets manquants.
  La capture reste stricte. Ce correctif attend sa propre qualification.
- L'auditeur compile et ses 9 tests passent (`benchmark-auditor-r4.log`) ;
  les 108 tests Node passent (`benchmark-node-r4.log`). La baseline a été
  reconstruite hors ligne depuis les 1 202 fichiers vérifiés de l'archive du
  commit de référence (`baseline-provenance-r2.json`). Aucune mesure réelle
  comparative ni supériorité de performance n'est encore établie.
- La revue critique du contrat L5 R3 ne relève plus de blocage P1/P2 de
  conception. Le premier lot autorisé porte sur le schéma privé 8 explicite,
  les refus des anciens lecteurs et les barrières du backend ordinaire.
  Les capacités de contrôle, lecture, activation, capture puis transitions
  gérées seront intégrées et qualifiées séparément. C13/C14 — récupération
  ordinaire gérée et élargissement de sélection — restent obligatoires.

## Qualification courante — acquisition R2, VFS R9 et Stage1a

- Acquisition Store : les cinq tests de politique, le test des deux façades de
  lecture et le scénario réel à deux processus passent. Ce dernier retient le
  writer pendant sept secondes, puis vérifie le refus CAS exact et une nouvelle
  capture explicite. La revue indépendante R2 ne relève aucun P0/P1/P2.
  Les délais restent de 60 s pour les appels ordinaires et de 5 s pour les appels
  courts, notamment l'acceptation gardée du pont Git. Voir
  `.layrs-local/validation/l1-acquisition-r2-root-handoff.md` pour les preuves
  et les limites de couverture des sites d'appel.
- VFS : les 26 tests provider, la garde de destination native, les deux tests
  client de partage/remplacement et treize recettes intégrées R9 passent.
  Les résultats incluent les exports de 70 Mio + 17 octets, les douze classes
  provider de renommage, les avances concurrentes, les reçus, les neuf
  interruptions d'activation et les destinations Unicode/8.3 réellement exercées.
  Les treize résultats client et leurs empreintes sont dans
  `l4-client-r9-results.json`; les sources sont dans `l4-r9-final-source.json`.
  La revue R9 clôt les défauts de garde et de casse en lecture statique, sans
  P1–P3. Les modifications de noms ou attributs par un writer actif restent hors
  de la garantie de quiescence. Les essais de contenu simultanément modifié et
  non résident restent **0/2 non qualifiés**, malgré un harnais qui se termine.
- Stage1a : la régression projection complète passe 155 tests; le lot ciblé
  révisé en passe 15. Les diagnostics singleton et préparation acquérante passent
  chacun leur reprise ciblée. Le test de mort du processus passe aux trois
  frontières de l'upgrade; le CLI historique vérifié accepte le schéma 7 puis
  refuse le schéma 8 sans écritures. Cela ne prouve ni les anciens handles
  conservés en mémoire, ni une coupure de courant. Les dix tests de routage liés
  et les 17 tests CLI offline + 9 tests partial repassent après Stage1a
  (`l8-recovery-binding-schema8-r1.log`, `l8-cli-schema8-r1.log`).
- Stage1b : le premier lot fournisseur est implémenté et ses nouvelles preuves
  natives passent ; les revues SPEC et critique R2 acceptent ce périmètre borné.
  La sonde initiale conserve
  l'échec du renommage avec le pin strict de FORMAT ouvert (erreur Windows 5),
  ainsi que le succès du contrôle avec le seul pin du dossier, de l'enregistrement
  CFAPI et de l'accès aux contrôles ordinaires sans callbacks. Le maintien d'un
  handle FORMAT partageant DELETE échoue également. La recette séparée fermant
  uniquement FORMAT passe ses six bras synchrones (`l5-control-close-window-r1.log`).
  La recette R2 passe également les contrôles d'interopérabilité des handles,
  d'enregistrement concurrent, de lecteur tiers, d'ACL, de collisions, de flux
  nommés et de liens (`l5-control-close-window-r2.log`, 1 test, 0,82 s).
  La tentative de mutation reparse est refusée avec l'erreur 1314; cette mutation
  reste non qualifiée. La revue critique R2 ne bloque pas le contrat borné;
  ses exigences de reprise, d'interopérabilité et de validation finale portent
  désormais sur les API de production : 33 tests de bibliothèque passent,
  ainsi que les huit groupes natifs et les huit interruptions avec reprises
  dans de nouveaux processus (`l5-control-step1-lib-r2.log`,
  `l5-control-step1-native-r1.log`, `l5-control-step1-process-r1.log`).
  La nouvelle mutation reparse avec GUID réussit réellement avant son refus
  par le guard. Les compteurs du test connecté restent à zéro pour les appels
  de métadonnées sous la politique ALWAYS_FULL testée. Cela n'établit aucune
  garantie sur les autres politiques de fournisseurs.
  Les créations privées Windows du Store passent sept tests, avec un test de
  symlinks explicitement ignoré (`l5-private-allocation-r6.log`). La dépendance
  Windows vers VFS est réservée aux tests ; trois recettes d'interopérabilité
  avec ces vrais allocateurs passent (`l5-store-provider-interop-r1.log`), avec
  revue indépendante finale R3 PASS sans P0/P1/P2. Aucun bootstrap SQL, accès source ou état
  prêt n'est autorisé par ces seules preuves de disposition.
  La découverte statique du bootstrap passe sa revue SPEC R2
  au niveau du contrat; elle n'est pas encore implémentée.
  Les capacités L5 de lecture, capture, récupération gérée et élargissement restent
  à implémenter et qualifier.
- Préparation Store Stage1b : les deux seams internes de parsing FORMAT et de
  validation structurelle SQL passent sept tests ciblés sur les sources finales
  (`l5-store-validation-seams-r2.log`). La suite projection complète passe
  165 tests, avec un test explicitement ignoré faute de lecteur CLI historique
  configuré (`l5-store-projection-after-seams-r1.log`). Les revues SPEC et critique
  ne relèvent aucun blocage. La conservation des protections nofollow et des
  erreurs de croissance du lecteur est vérifiée par comparaison de source,
  sans nouvelle preuve native de ces deux chemins. Le manifeste final
  `l5-store-validation-seams-final-source.json` distingue les corrections
  cosmétiques du test et les sources réellement exécutées. Aucun writer de
  bootstrap, droit source ou passage à ready n'en découle.
- Stage2 fournisseur : après les checkpoints 38 puis 39 tests, R1 passe
  41 tests de bibliothèque, avec cinq recettes explicitement ignorées par le
  filtre normal. Les quatre recettes natives ciblées passent : création/audit
  READY, refus de population, cinq coupures de processus et rétention après
  échec de déconnexion injecté. Les sources, le binaire et ces cinq journaux
  sont liés par `l5-provider-stage2-r1-source.json`. La mise à jour READY réussit
  avec les handles d'audit conservés et les requêtes de registration sur handles
  retenus et réouverts. Neuf recettes historiques fournisseur/Step1 repassent
  (`l5-provider-stage2-regressions-r1.json`). Les revues SPEC et critique R1
  demandent néanmoins des corrections et des preuves supplémentaires ; ce
  checkpoint ne clôt pas Stage2. Le test ajouté de casse reproduit un défaut
  réel après registration INCOMPLETE : Stage1 accepte le chemin, mais Stage2
  traite son pin racine comme un ancêtre
  (`l5-provider-stage2-case-red-r1/result.json`). R2 corrige la sélection du pin
  racine et compare explicitement l'identité native de la racine Session avec
  celle de la preuve. Le même test de casse passe. Les 41 tests de bibliothèque
  passent (11 recettes ignorées par ce filtre), ainsi que les dix recettes
  natives ciblées : audit READY, casse, durée de vie indépendante, annulation et
  déconnexion, refus initiaux, cinq coupures de processus, attributs et
  registration imbriquée, chemin de 311 unités UTF-16, prédicats résident/modifié
  et mutations de métadonnées. Le dernier test passe après trois échecs de
  construction de fixture conservés ; il réalise réellement le reparse GUID et
  exige le refus exact du fournisseur. Les prédicats résident/modifié utilisent
  une fixture READY hydratée, pas une hydratation dans la création source-free.
  Les vérifications de bibliothèque de production VFS et client passent aussi.
  `l5-provider-stage2-r2-final-source.json` conserve les sources et binaires de
  chaque état, les échecs et la portée précise des réexécutions. Les nouvelles
  revues SPEC et critique R2 ne relèvent aucun P0/P1/P2 restant. Elles retiennent
  trois limites de preuve P3 : quelques anciens journaux n'enregistrent pas le
  hash du binaire exécuté ; le fresh-open du test de durée de vie n'est essayé
  qu'après le drop ; l'attribution du refus reparse combine la fixture et
  l'ordre vérifié du code, car le message d'erreur n'est pas unique.
  La déconnexion échouée est injectée ; la connexion native encore active
  empêche d'attribuer son refus DELETE sur la racine aux seuls pins retenus.
- Stage1b : le contrat R4 de bootstrap Store est accepté en conception par SPEC
  et critique, jusqu'à `control_published` avec famille `initializing`. Il fixe
  les trois reçus, les encodages et la reprise sans réinitialisation ni réécriture
  de FORMAT. Les records purs sont raccordés uniquement sous `cfg(test, windows)` :
  première compilation réussie, 14 tests passent et un échoue sur l'hypothèse de
  sérialisation des grands entiers. Après correction de cette hypothèse et
  alignement exact des fixtures R4, les 17 tests passent ; 58 champs candidats
  pour les goldens sont conservés avant gel en constantes. Les revues SPEC et
  critique les reconstruisent indépendamment et les valident sans changement.
  Elles demandent des cas purs pour les branches acceptées et les frontières de
  budgets, ainsi que des contrôles plus complets des engagements d'allocation,
  de descripteurs et d'éligibilité terminale. Le correctif doit préserver la
  distinction entre liaison des reçus et validation SQL encore en attente :
  une base chaude ne fournit pas de mount validé ni de phase acceptée.
  Le correctif pur R3 est accordé dans les records/reçus, comparaisons de lignes,
  tests et un fixture littéral ; la liaison terminale retourne explicitement
  `TerminalPendingValidation` jusqu'à la comparaison avec l'état SQL validé.
  Le gel R3 compile et ses 29 tests ordinaires passent (six tests lourds
  explicitement ignorés dans cette commande). Les six cas purs de frontières
  passent ensuite séparément sur le même binaire conservé, dont les empreintes
  sont vérifiées avant et après chaque exécution. Le cas d'agrégat dure 308 s
  avec un pic de working set mesuré de 286 Mio ; ces mesures caractérisent le
  test, pas les performances de production. Le cas de 100 000 descripteurs
  reste une preuve pure, sans qualification de 100 000 fichiers natifs. Le
  [manifeste R3](../../.layrs-local/validation/l5-store-stage1b-pure-records-r3/review-manifest.json)
  conserve 22 témoins source et 41 artefacts ; les nouvelles revues SPEC et
  critique acceptent ce lot pur sans constat bloquant. Deux P3 de couverture
  restent affectés aux fixtures de validation live : mutations indépendantes
  des identités/digest du Marker et contrôle positif d'un fichier exécutable.
  Le plan du premier incrément
  live — inspection et rollback explicitement lié, sans writer — est soumis à
  des revues SPEC et critique distinctes avant tout accord d'implémentation.
  La SPEC demande d'inclure les réexports Windows de `projection.rs`. L'inventaire
  DDL courant comprend 39 tables privées : seules les huit tables du bootstrap
  peuvent être peuplées, avec leurs contenus exacts ; les 31 autres doivent être
  vides, notamment `projection_content` et `projection_observations`. La portée
  live doit aussi prévoir la conservation des causes natives originales.
  La critique du plan impose des sondes nofollow des noms fixes `-journal`,
  `-wal` et `-shm` avant SQLite pour les deux bases. Le code SQLite embarqué
  confirme qu'une connexion readonly peut supprimer des sidecars d'une base
  vide ou ouvrir un WAL malgré l'en-tête. Une base vide ne doit donc jamais
  être connectée ; seule l'absence vérifiée de tout sidecar permet de décrire
  l'état privé non initialisé. WAL/SHM présents refusent toute connexion.
  Le handle de reprise libère tous les `StoreLock` du constructeur et ne garde
  que les pins/preuves natives. Chaque méthode reprend les verrous dans l'ordre,
  avec catalogue exclusif dès la sonde de récupération, sans upgrade ni entrée
  imbriquée. Le plan R2 précise que les handles de sondage des sidecars ne
  demandent pas DELETE et sont fermés avant chaque connexion, tandis que les
  pins de base et de parent restent détenus. Il prévoit aussi une variante
  d'erreur Windows conservant les causes typées originales et les effets de
  rollback, sans changer les conversions ordinaires. La SPEC R2 demande de
  rendre explicite la décision de registre portée par T : `absent` ne permet
  que l'absence fraîchement validée ; toute apparition de ligne est refusée.
  `initializing` permet la ligne initialisante exacte et éligible, ou l'absence
  validée. La validation familiale et les dépendances restent indépendantes de
  la validité des lignes privées. Toute sortie après une tentative de rollback
  doit exposer ses effets terminés ou incertains avec la cause originale, y
  compris dans un objet d'erreur. La critique R2 confirme ces dispositions mais
  relève deux P1 avant toute reprise writable : un journal peut restaurer un
  en-tête WAL, puis faire ouvrir/créer le WAL pendant le premier read ; une
  référence de super-journal peut provoquer des accès et une suppression hors
  des trois noms sondés. Ces chemins sont confirmés dans SQLite embarqué.
  Le plan doit donc prouver avant replay l'absence de ces effets, sans suivre
  de référence externe ni modifier les modes SQLite. Une disposition bornée
  supplémentaire est demandée en lecture seule ; le lot live reste bloqué.
  La proposition R3 ajoute un parseur de journal strict et streaming avant RW,
  avec rejet des signatures de super-journal, de toute préimage de page 1 en
  WAL et des formes ambiguës. Elle prévoit 256 Mio/524 288 records/4 096 headers
  comme budgets d'admission du journal, sans plafond physique de base, ainsi
  qu'une vérification de la géométrie native SQLite. Certains journaux SQLite
  légitimes resteraient volontairement non pris en charge. La SPEC R3 accepte
  ce sous-ensemble conservateur et la matrice terminale, sans nouveau blocage
  de conception. La critique R3 accepte aussi la fermeture des deux effets de
  replay sous les hypothèses Win32 figées et de verrous coopératifs. Elle exige
  un opener provider nu : le `PRAGMA synchronous=FULL` de l'opener ordinaire
  peut déjà déclencher le replay avant le retour de connexion. La géométrie
  doit être comparée avant tout SQL déclencheur, avec contrôle du profil VFS,
  du cache partagé et des auto-extensions. Le cas secteur initial 4 096,
  secteur/page journal 512 et EOF 1 032 reste un négatif explicite à qualifier.
  Les refus devront tracer l'absence d'ouverture RW et d'accès externe ; des
  témoins d'octets inchangés ne suffisent pas. La nouvelle
  voie doit éviter `lease`/`catalog_once` et leurs conversions `map_err(invalid)`
  afin de conserver les erreurs de verrou d'origine. Aucun résultat de reprise
  n'est encore crédité à ce plan.
  Le préalable de dépendance est réalisé par ROOT : `layrs-vfs` devient une
  dépendance Windows normale du Store et le verrou Desktop reçoit ce lien ;
  le verrou racine le contenait déjà et reste inchangé. Les graphes Windows
  racine/Desktop passent en mode locked/offline, ainsi que `cargo check` du
  Store (16,51 s, six avertissements existants). La vérification exhaustive
  hors ligne et l'arbre Linux échouent faute de paquets en cache ; aucune
  compilation Linux n'est revendiquée. Le diff exact et les témoins figurent
  dans [le manifeste de dépendance](../../.layrs-local/validation/l5-store-vfs-normal-dependency-r1-9664d886337c4c72bcbda89762b1ab57/review-manifest.json),
  accepté par la revue indépendante sans constat P0–P3. Cette étape de dépendance
  n'accordait aucun câblage live ni writer ; le profil SQLite restait à préciser
  avant le lot d'implémentation décrit ci-dessous.
  La disposition suivante distingue les contrôles exécutables (cache privé,
  chemin natif sans URI, VFS nommé, build/géométrie) de la précondition d'hôte :
  SQLite ne permet pas d'établir publiquement l'absence d'auto-extensions avant
  `open_v2`. Une déclaration explicite d'hôte audité, désactivée par défaut,
  est acceptée en conception par SPEC/critique R4. Elle ne serait ni une preuve
  de registre vide ni une autorité sur les fichiers. Les causes de fermeture
  préservées sont celles exposées par SQLite : son `sqlite3OsClose` peut masquer
  un échec natif, et `SQLITE_OK` ne prouve pas la fermeture de tous les handles.
  L'audit doit couvrir aussi la configuration initiale des mutex, les overrides
  d'appels système et les callbacks personnalisés. Correction du rapport
  critique R4 : cette source expose les définitions conventionnelles
  `EXTRA_INIT`/`EXTRA_AUTOEXT` dans ses options compilées ; cela ne prouve ni la
  fermeture du code natif lié ni l'absence de callbacks enregistrés. Les
  contrôles précèdent SQL et replay ; l'ouverture nue lit
  déjà l'en-tête. L'intégration au démarrage
  CLI/Desktop reste exclue tant que leur profil natif n'est pas qualifié.
  L'audit source des deux hôtes ne trouve pas de hook SQLite personnalisé dans
  les chemins applicatifs inspectés, mais ne qualifie aucun des deux binaires.
  Les graphes sauvegardés diffèrent sur `unlock_notify` ; ils ne prouvent pas
  les features de l'unité de build CLI ni le linkage final. Les sorties SQLite
  en cache ne consignent pas tous les overrides de compilation. Desktop ajoute
  les initialisations pré-main et WebView2 ; son `.setup` arrive après la
  création des fenêtres et serait trop tardif. La qualification devra lier
  chaque déclaration au build exact et aux essais en processus isolés, avant
  tout appel de démarrage ; la bibliothèque client ne déclare pas pour son hôte.
  Le lot préalable accorde alors le seul parseur pur de journal accepté en R3 :
  nouveau module et tests, câblage `cfg(all(test, windows))`, sans connexion,
  FFI, fichier natif ou effet de reprise. Le gel R1 compile en 20,56 s ; ses
  29 tests ordinaires passent, puis le test exact de flux virtuel de 256 Mio
  passe seul sur le même exécutable (0,735 s native, pic working set 6 746 112
  octets). Deux passages streaming traitent environ 512 Mio ; ce journal
  synthétique ne constitue pas une preuve d'émission par SQLite. Les 19 témoins
  restent inchangés et les 32 artefacts sont conservés dans le
  [manifeste du parseur pur R1](../../.layrs-local/validation/l5-store-stage1b-pure-journal-r1/review-manifest.json).
  Les revues SPEC et critique acceptent ce périmètre pur sans constat, après
  vérification des 30 cibles, des empreintes et des définitions SQLite épinglées.
  Aucune reprise native n'est encore qualifiée. Le premier incrément live est
  maintenant accordé dans la portée revue : validation SQL complète, inspection,
  récupération explicitement liée et déclaration de profil désactivée par
  défaut. Les corps purs restent figés, sauf visibilité/câblage nécessaires.
  Le gel first-live R1 compile en bibliothèque Windows et en binaire de tests.
  Les trois nouveaux tests purs, les 29 tests records et les 29 tests journal
  passent ; sept cibles de ressources restent ignorées sur ce binaire.
  Les 80 témoins source sont inchangés et les neuf inverses préexistants exacts.
  Le [manifeste first-live R1](../../.layrs-local/validation/l5-store-stage1b-first-live-r1-8a622825d25440da9072111bd7353364/review-manifest.json)
  conserve les sources, le diff, le binaire et les journaux. Les premières
  captures Cargo ont perdu le code de sortie du lanceur ; leurs logs sont
  conservés, et deux contrôles incrémentaux avec capture corrigée terminent à 0.
  Les revues SPEC et critique demandent des corrections. Un P1 autorise la
  reprise familiale malgré une contradiction privée connue ; quatre P2 de code
  concernent le filtre de schéma avec joker, la perte du déclencheur après
  rollback, le VFS de test non sélectionné et une assertion de code natif effacé
  par le helper. Des tests SQL/familiaux indépendants et des échecs après effet
  restent aussi à ajouter. Le correctif R2 est accordé dans les seuls modules
  first-live et leurs tests ; les preuves R1 restent intactes. Les 24 recettes
  ignorées et leur helper compilent, mais leur exécution reste suspendue.
  Le gel R2 modifie 14 fichiers et ajoute quatre modules de tests ; les 66
  autres témoins R1 restent identiques et les inverses sont vérifiés en mémoire.
  ROOT compile la bibliothèque (7,63 s Cargo) et les tests (23,94 s Cargo),
  avec les deux codes de sortie à 0. Six tests purs first-live, 29 records et
  29 journal passent sur le même binaire retenu ; les sept cibles de ressources
  restent ignorées. Les 84 sources/copies sont inchangées après ces contrôles.
  Le [manifeste first-live R2](../../.layrs-local/validation/l5-store-stage1b-first-live-r2-91878fdd0a90401db5891070c0ec9b78/review-manifest.json)
  conserve 45 artefacts. Les nouvelles revues SPEC et critique sont lancées ;
  les 38 recettes SQL/profil/natives et le helper restent non exécutés.
  La liste des déclencheurs typés et le partage par `Arc` sont une évolution
  explicite des données causales de l'API R1, sans changement durable R4.
  La revue critique R2 ferme les constats de production de son périmètre,
  mais bloque une recette P2 : l'ajout d'un octet au reçu atteint la borne
  de taille avant la comparaison d'octets attendue. ROOT confirme ce premier
  refus ; le correctif proposé garde la mutation et les assertions d'effets
  et de déclencheurs, puis exige la cause typée exacte de dépassement de borne.
  SPEC R2 ne trouve plus de P0–P2 bloquant, mais signale un P3 : le raccourci
  `readonly_rollback` peut reprendre le déclencheur privé pour une erreur
  familiale. Les deux originaux restent correctement associés dans la liste
  complète ; les décisions de récupération ne sont pas affectées. ROOT accorde
  un correctif R3 dans trois fichiers : assertion typée exacte de la borne,
  sélection implicite du déclencheur par base, puis contrôles purs ciblés dans
  le test existant. Les 81 autres témoins R2, les encodages et les décisions
  de récupération restent figés. Le gel R3 est vérifié : trois différences
  bornées et leurs inverses R2 exacts. ROOT compile la bibliothèque (2,13 s
  Cargo) et les tests (8,79 s Cargo), avec exit 0, puis les six cibles pures
  first-live passent chacune une fois, dont le test étendu. Les 84 témoins
  restent inchangés ; les suites records/journal ne sont pas réexécutées et
  conservent leur portée de preuve sur R2. Le
  [manifeste first-live R3](../../.layrs-local/validation/l5-store-stage1b-first-live-r3-bdd774e4086b4e519ebbd364368c2cc6/review-manifest.json)
  conserve 39 artefacts. Les deux revues indépendantes SPEC et critique R3
  acceptent la correction au niveau source, sans nouveau constat P0–P3.
  Les défauts restants de portée causale et d'assertion de borne sont clos.
  Les 38 recettes et le helper n'avaient pas encore été exécutés au gel.
  L'audit statique retrouve le PDB correspondant au binaire R3 ; ROOT
  en conserve une copie identique (SHA256 A69F6E31…A73E5, 185 208 832 octets),
  vérifie 36 entrées de provenance et conserve 30 copies supplémentaires.
  Neuf sources officielles Rust 1.85 au commit immuable sont retenues avec
  leurs empreintes présentes dans le PDB correspondant. L'audit de démarrage
  est clos : harness standard, initialisation CRT/Rust et SQLite statique
  concordent. ROOT accepte cet hôte libtest possédé sans rebuild, sous
  confiance explicite dans les composants stock Rust/MSVC et le runtime
  Windows. Les anciens CL/_CL_ effectifs et les DLL réellement chargées
  restent non vérifiés ; aucune preuve universelle de registre vide ou
  qualification de l'hôte produit n'est revendiquée. Cette disposition
  précède les recettes de profil, qui ne prouvent pas leur propre précondition.
  La première file R3 exécute exactement trois recettes de profil puis trois
  recettes SQL en mémoire, chacune dans un processus et son enfant possédé.
  Les trois profils passent (SQLite 3.46.0, 51 options) et les deux premiers
  cas SQL passent. Le sixième lancement termine exit 101 : le manifeste []
  est refusé, mais le test attend « invalid type » tandis que le décodeur
  retourne « invalid length 0, expected struct FileManifestWire with 6 elements
  at line 1 column 2 ». La file s'arrête sans retry ; les 32 recettes
  restantes, les cas ultérieurs de ce test et le helper d'interruption ne
  sont pas qualifiés. Aucun parent ni fixture disque first-live n'est créé.
  Les [preuves du premier lancement R3](../../.layrs-local/validation/l5-store-stage1b-first-live-r3-bdd774e4086b4e519ebbd364368c2cc6/owned-runtime-9e570fd60b28437b8c931f906ca9ddc3/runtime-review-manifest.json)
  conservent 23 artefacts. Leur revue indépendante accepte le supplément
  d'hôte sous la confiance explicite retenue, confirme les cinq réussites
  et l'arrêt sur l'assertion P2, sans défaut de production supplémentaire.
  Les cas ultérieurs du test échoué demeurent non qualifiés. Une correction
  R4 source-only est accordée dans le seul validation/tests.rs : diagnostic
  exact pour la même mutation, sans autre cause acceptée ni changement du
  validateur. Le gel R4 ne change qu'une ligne, avec inverse R3 exact et
  83 témoins inchangés. ROOT compile les tests avec exit 0 (6,37 s Cargo,
  22 215,187 ms au lancement) ; l'inventaire reste 335 tests, dont les
  45 cibles first-live. Aucun corps de test R4 n'est exécuté à ce stade. Le
  [manifeste first-live R4](../../.layrs-local/validation/l5-store-stage1b-first-live-r4-920f024774094066ac9237fa3f296700/review-manifest.json)
  conserve 20 artefacts, le nouvel EXE et son PDB correspondant (age 12).
  Les 36 copies de provenance R3 sont préservées ; 33 entrées originales
  restent identiques, hors PDB et deux métadonnées de build actualisées.
  Les revues SPEC/critique R4 acceptent cette correction sans constat P0–P3.
  ROOT accepte le nouveau binaire dans le même périmètre d'hôte audité,
  avant exécution. La recette SQL corrigée et les trois suivantes passent,
  puis les cinq recettes directes d'accès DB passent. Le premier cas de
  récupération échoue pendant la préparation : BEGIN IMMEDIATE dans
  validation/tests/fixture.rs retourne SQLite CannotOpen, code étendu 14.
  Les assertions d'inspection ne sont pas atteintes. La file s'arrête après
  neuf réussites et cet échec ; les 23 recettes restantes sont non exécutées.
  Les [preuves natives R4](../../.layrs-local/validation/l5-store-stage1b-first-live-r4-920f024774094066ac9237fa3f296700/owned-runtime-298b71802a2b44dfa2e81575e117ec94/runtime-review-manifest.json)
  conservent 37 artefacts et les cinq chemins de fixtures. Le premier relevé
  ROOT avait manqué quatre chemins préfixés par libtest ; une adjudication
  depuis les sorties brutes les conserve, sans rerun ni modification des
  résultats originaux. Aucun code Windows sous-jacent n'est déduit du code
  SQLite 14. La revue runtime indépendante accepte les neuf réussites dans
  leurs portées respectives et la file arrêtée, sans nouveau constat P0–P3.
  Elle confirme la chronologie de qualification de l'hôte, les deux sorties
  enfant/parent par cible, l'adjudication des chemins et l'échec avant toute
  assertion d'inspection. L'audit causal confirme que l'ouverture DB et le
  mode DELETE précèdent le premier BEGIN IMMEDIATE, qui peut créer le journal.
  Le chemin privé calculé depuis le log mesure 255 unités UTF-16, son journal
  263 ; cette longueur est une hypothèse principale, pas une cause Win32
  établie. Le diagnostic test-only R5 est gelé dans le seul
  validation/tests/fixture.rs : errno SQLite capturé immédiatement après
  execute_batch, puis nom disponible et longueurs lexicales, sans nouvelle
  requête ou sonde. Le Result original et son unwrap sont conservés ; aucun
  chemin, droit, partage ou SQL ne change. L'inverse R4 est exact et les
  83 autres témoins restent inchangés. ROOT compile les tests avec exit 0
  (9,69 s Cargo ; 30 310,3717 ms au lancement), retient l'EXE et son PDB
  correspondant age 13, et retrouve les 335 tests dont 45 cibles first-live.
  Le [manifeste first-live R5](../../.layrs-local/validation/l5-store-stage1b-first-live-r5-a2a94f19a717486b998b1b6a5d339761/review-manifest.json)
  conserve 20 artefacts. Les nouvelles revues SPEC/critique acceptent la
  source ; SPEC relève uniquement une erreur P3 dans la note annonçant
  120 s au lieu des 180 s inchangées du wrapper. Un erratum ROOT conserve
  la note figée et retient 180 s. Après disposition explicite d'hôte R5,
  un seul lancement diagnostique conserve l'échec CannotOpen/14 et les
  sorties enfant/parent 101, avant inspection. Le relevé expose errno 3,
  un nom DB de 255 unités UTF-16 et un journal dérivé de 263 ; il ne prouve
  pas la première erreur Win32 ni la cause du refus. Les
  [preuves diagnostiques R5](../../.layrs-local/validation/l5-store-stage1b-first-live-r5-a2a94f19a717486b998b1b6a5d339761/diagnostic-runtime-4a21d7b86df147e887f6c970d49e133a/runtime-review-manifest.json)
  conservent six artefacts, sans retry, nettoyage ou reprise de la file.
  La revue runtime indépendante accepte ce relevé issu d'un cas échoué,
  sans constat P0–P3. Elle confirme les 14 champs, les longueurs et le
  maintien du refus original ; aucune qualification de récupération n'en
  découle. ROOT accorde un R6 limité à recovery/tests/fixture.rs : étiquette
  de racine raccourcie et budget lexical de noms SQLite, avant la première
  création SQL sur disque. Chaque nom, ses suffixes déterministes et le
  NUL doivent tenir dans 260 unités UTF-16, avec une réserve de 12 unités
  pour le super-journal ; un parent hors budget échoue explicitement.
  Cette configuration de test ne démontre pas la cause historique et ne
  change aucune politique de production. SQL, handles, diagnostic R5,
  assertions, cibles et délais restent préservés ; compilation et revues
  fraîches sont requises avant toute nouvelle décision d'exécution.
  Le gel R6 ne modifie que recovery/tests/fixture.rs ; l'inverse R5 est
  exact et les 83 autres témoins sont identiques. ROOT compile les tests
  avec exit 0 (8,61 s Cargo ; 29 343,9046 ms au lancement), retient l'EXE
  et son PDB correspondant age 14, puis retrouve les 335 tests et les
  45 cibles first-live inchangées. Le
  [manifeste first-live R6](../../.layrs-local/validation/l5-store-stage1b-first-live-r6-a9898c82a415455c9b24656b5dd7e86d/review-manifest.json)
  conserve 20 artefacts et 84 copies source. Les 36 entrées d'hôte retenues
  et 33 entrées originales applicables concordent ; cette continuité ne
  remplace pas une nouvelle disposition d'hôte. Les revues SPEC/critique
  acceptent le changement sans constat P0–P3. L'inventaire des cibles est
  identique sémantiquement ; sa sérialisation JSON diffère, tandis que la
  sortie libtest est identique octet par octet. ROOT accepte explicitement
  le nouvel hôte R6 dans le périmètre de confiance déjà documenté, puis
  prépare les ordinaux 15–38 en comparant chaque nom et délai au plan
  d'exécution R3 approuvé. Cette file est distincte de l'ordre déclaratif
  des tests et s'arrête au premier résultat non réussi. Les ordinaux 15–29
  passent : inspections répétées, contrôles familiaux, reprises DELETE
  réelles privée/familiale/double, refus WAL/super-journal, effets conservés
  et limites de fermeture/géométrie. Les 16 sélections totalisent
  90 511,2113 ms au lancement et retiennent 29 chemins de fixture.
  L'ordinal 30 échoue dans le helper triggers : il exige encore
  readonly_rollback.is_some() pour une erreur privée dont le seul trigger
  appartient à la famille. La sélection par base corrigée en R3 prévoit
  précisément None dans ce cas. Les assertions de portée, cause schema8,
  effets vides et vecteur familial ont précédé l'échec ; les contrôles
  suivants de zéro ouverture RW et de témoins inchangés restent non atteints.
  Les [preuves natives first-live R6](../../.layrs-local/validation/l5-store-stage1b-first-live-r6-a9898c82a415455c9b24656b5dd7e86d/owned-runtime-f82712c709694a56952d125c6ecfda84/runtime-review-manifest.json)
  lient 52 artefacts et six entrées. La file s'arrête sans retry ni nettoyage,
  avec huit recettes 31–38 non exécutées. La revue runtime indépendante
  accepte les 15 réussites bornées et l'arrêt conservé ; elle confirme
  un P2 d'attente du helper, sans défaut production identifié. Elle
  distingue les compteurs réellement contrôlés des seuls effets déclarés,
  les observations VFS de toute garantie de confinement, et la suppression
  d'une erreur de fermeture injectée après une fermeture réelle réussie.
  L'audit source confirme l'attente obsolète du helper et
  établit les six raccourcis attendus explicitement : None, index 0,
  index 0, index 1, index 1, index 1. Le correctif proposé exige aussi
  l'identité de l'erreur pointée et la portée DB attendue, tout en gardant
  les contrôles de vecteur, d'effets, de causes et de zéro RW. Après cette
  revue, ROOT accorde R7 dans le seul interruption/r2.rs et une nouvelle
  note de gel : attentes littérales, identité du trigger et portée DB
  explicite, sans modification production. Les preuves R6 restent intactes ;
  compilation, revues fraîches et disposition d'hôte précéderont toute
  nouvelle exécution. Aucune acceptation globale n'est anticipée.
  Le gel R7 ne change que ce fichier de test ; ROOT vérifie l'inverse
  exact vers R6 et les 83 autres témoins, puis compile avec exit 0
  (12,85 s Cargo ; 33 958,0969 ms pour le lancement). L'EXE retenu et
  son PDB correspondent au GUID observé, âge 15. L'inventaire sort
  avec exit 0 : 335 tests, dont les 45 cibles first-live inchangées.
  Les [preuves de compilation R7](../../.layrs-local/validation/l5-store-stage1b-first-live-r7-c7a9123458e046129d5c837433459015/review-manifest.json)
  conservent 84 sources et 20 artefacts, avant toute exécution de corps
  de test R7. Les deux nouvelles revues indépendantes SPEC et critique
  concluent SOURCE-PASS, sans nouveau P0–P3, et clôturent le P2 en source.
  Après vérification des 36 entrées d'hôte conservées et des 33 originaux
  applicables, ROOT accepte explicitement l'EXE R7 comme hôte de test
  dans le périmètre de confiance Rust/MSVC/Windows déjà documenté.
  L'échec R6 de l'ordinal 30 et ses contrôles zéro-RW/témoins non
  atteints restent inchangés dans les preuves R6.
  ROOT exécute ensuite les neuf recettes 30–38 sur un parent neuf,
  chacune une fois, dans la tranche exacte du plan original approuvé.
  Le pilote vérifie les noms ordonnés, les arguments et les empreintes
  avant chaque lancement. Les neuf recettes réussissent avec exit 0,
  stderr vide et deux résumés de réussite chacune, enfant puis parent.
  L'ordinal 30 atteint désormais ses contrôles de zéro ouverture RW
  supplémentaire comptée et de témoins inchangés. Les cas suivants
  exercent les causes après effets terminés, le refus géométrique réel
  4096/512/1032 et les décisions terminales, dans les limites de leurs
  assertions. Les 12 helpers d'interruption sont des coupures contrôlées
  par les parents, pas des cibles de réussite supplémentaires.
  Les [preuves natives R7](../../.layrs-local/validation/l5-store-stage1b-first-live-r7-c7a9123458e046129d5c837433459015/owned-runtime-2fe234ab60f849a291a7e936120d04d4/runtime-review-manifest.json)
  conservent 31 artefacts, six entrées épinglées et neuf chemins de
  fixtures distincts ; les durées extérieures totalisent 41 371,4514 ms.
  Aucun retry, nettoyage ou avertissement de fermeture n'est enregistré.
  La revue indépendante accepte les neuf réussites bornées, sans nouveau
  P0–P3, et clôture le manque de preuve de l'ordinal 30 dans ce périmètre.
  Précision de l'ordinal 35 : les assertions comptent cinq ouvertures de
  bases principales et vérifient que la dernière est writable ; elles ne
  comparent pas le nom et le mode de chaque événement. Le code établit
  l'ordre attendu et l'arrêt sans probe après l'échec de fermeture BUSY.
  À l'ordinal 38, l'inspection après libération du handle atteint
  AbandonIncomplete et le témoin du sidecar reste inchangé. Les réussites
  R3, R4 et R6 restent attachées à leurs binaires ; il ne s'agit pas d'une
  exécution des 38 recettes sur R7. Aucune acceptation globale ni activation
  produit ne découle de cette seule tranche R7.
  ROOT demande une proposition en lecture seule du prochain incrément
  provider, avec dépendances, périmètre et limites explicites. La proposition
  reçue vise la création jusqu'à control_published, avec reprise de publication
  et abandon strict sans dépendances acceptées ; le membre resterait initializing.
  Avant les revues de conception, un addendum doit fixer les types publics de
  compte rendu des écritures, la conservation des gardes et le traitement des
  commits/fermetures incertains. Aucun writer ni nouvel essai n'est accordé.
  Les sources restent gelées pendant cette préparation. Les preuves de récupération
  qualifient seulement les cas et hôtes de test décrits ; elles n'activent
  aucune récupération dans les hôtes produit.
  Aucun writer, abandon, publication ou appel de déclaration au démarrage
  CLI/Desktop n'est accordé.
  Les observations A+B compilent, avec 44 tests de bibliothèque réussis. Le
  défaut de lettre de lecteur est reproduit nativement puis corrigé localement
  par comparaison `OsStr` exacte, sans changer le corps du test rouge/vert.
  Sept recettes natives passent, dont le refus de renommage après persist avec
  observateur retenu et la reprise après libération. Deux recettes restent non
  qualifiées : privilège symlink absent (1314), et refus de type de racine avant
  la requête NOT_UNDER attendue. Leurs branches suivantes n'ont pas été exécutées.
  Les checks de production VFS/client et trois régressions Stage2 passent ;
  les preuves sont dans `l5-provider-observation-r2`, les échecs précédents sont
  conservés. Les revues SPEC et critique R2 ne trouvent plus de défaut de
  production. Les onze recettes indépendantes passent ensuite sur le binaire
  retenu R3 (44 tests de bibliothèque, 31 ignorés) ; SPEC et critique approuvent
  cette qualification bornée sans P0–P2. Le refus delete-pending est observé
  avec le même code/kind natif direct ; son attribution à `final_absent` reste
  inférée, et le témoin d'octets est un fichier séparé (P3).
  Les trois refus de type précoces ne sont pas des preuves de requête NOT_UNDER
  exécutée dans A/B. Le symlink 1314 reste non qualifié, et les deux anciens
  agrégats restent en échec. B conserve la comparaison de chemin Step1, insensible à
  la casse de la lettre du lecteur : l'égalité exacte des chaînes du reçu reste
  une vérification Store distincte. Clippy strict reste en échec, avec des
  diagnostics antérieurs et des diagnostics de tests propres à l'observation.
- Régression client après Stage2 R1 : le parcours à deux racines, la
  capture/export après remplacement de casse et les neuf frontières de reprise
  durable passent (`l5-stage2-client-regressions-r1.json`). Le test d'export de
  70 MiB + 17 octets échoue au délai existant de 60 secondes de son enfant de
  lecture, avant l'export. Une seconde exécution du même binaire conservé
  reproduit cet échec (`l5-stage2-client-large-diagnostic-r2/`). Le journal R9
  antérieur reste une preuve historique, pas une réussite du code actuel.
  Un diagnostic isolé conserve le délai de 60 secondes et échoue également.
  Il mesure 153 plages achevées, 40 108 032 octets transférés et une progression
  continue ; environ 59,55 secondes sont dans la lecture/validation source,
  contre 0,33 seconde dans CfExecute. Il ne résout pas l'écart avec R9.
  Une correction de test uniquement porte à 180 secondes le premier enfant
  d'hydratation de ce grand cas ; les dix autres appels restent à 60 secondes,
  avec assertions et arrêt/récolte des enfants inchangés. Sa revue indépendante
  SPEC/qualité ne relève aucun problème. L'exécution fonctionnelle R2 passe en
  341,40 secondes, avec son enfant d'hydratation en 127,22 secondes, puis les
  assertions de capture/export et le cas relocalisé. Le journal
  `l5-stage2-client-large-functional-180-r2.log` conserve la commande et l'exit 0.
  Ce succès sous le budget ciblé ne constitue pas une correction de performance
  et n'explique pas l'écart avec R9.
- Pilote partagé R6 : `layrs-worktrees-GI8ZzO/report.json` reste **FAILED**.
  Les 32 commandes CLI, les octets privés et les historiques 3/5 passent, mais
  l'audit trouve deux Views non indexées issues de tentatives CAS périmées et un
  Tree historique omis du calcul de rétention. Les corrections ciblées passent
  23 tests write-batch, 26 fsck, puis les tests client précontrôle (1), rétention
  (3), concurrence (3) et acquisition (1, avec son helper ignoré séparément).
  La revue finale R2 ne relève aucun P0/P1/P2. Le nouveau candidat et l'auditeur
  sont reconstruits depuis 784 sources vérifiées. Le nouveau pilote R7
  `layrs-worktrees-UFmI6b/report.json` passe ses 32 commandes et l'audit strict :
  100 objets indexés et physiques, 79 378 octets, aucun objet inaccessible.
  Le pilote `layrs-worktrees-peAfGZ/report.json` passe aussi les cellules assets
  et mixte : 32 commandes chacune, respectivement 100 objets / 208 484 octets
  et 108 objets / 155 515 octets, avec réconciliation exacte et zéro inaccessible.
  Les mêmes cibles passent également sous Docker et Ubuntu natif, avec
  57 tests substantiels par environnement et provenance des snapshots conservée.
  Voir [les mesures](worktrees-partial-vfs-benchmarks.md) pour les pins et limites.
  Ces trois profils à 32 fichiers et deux agents ne qualifient ni les matrices complètes
  ni une supériorité de performance. Le pilote R6 échoué reste inchangé.
- Candidat release R7, code 1 000 fichiers, un agent en copies indépendantes :
  `layrs-worktrees-Ndr3co/report.json` reste **FAILED**. Dix commandes réussissent,
  puis le budget global de 30 minutes arrête le second statut sale (commande 11,
  `timedOut=true`) ; l'audit final n'a pas commencé. Le runner sort avec le code 1,
  ses processus sont arrêtés et les empreintes CLI/auditeur sont inchangées.
  Cette tentative sur les binaires R7 figés ne fournit ni cellule complète
  comparable ni gain de performance. Rapport, logs et fixture sont conservés ;
  le diagnostic des coûts précède toute nouvelle série longue.
- Le diagnostic de statut utilise deux copies isolées des sources baseline/R7,
  avec le même collecteur et sans optimisation. Sa revue R1 demande de fermer
  une course d'activation/finalisation du collecteur et de rendre les tests
  d'invalidation et d'attribution discriminants avant toute mesure. Les copies
  R1 restent conservées. Le premier build baseline échoue dans MSVC sur des
  chemins de sortie de 260 caractères ou plus ; le build release réussit ensuite
  dans un target plus court (1 min 53 s), sans changement de sources ni de
  paramètres Windows. Les 1 203 empreintes source sont revérifiées après build,
  et les deux exécutables sont conservés. Aucun test du collecteur ni aucune
  mesure sur fixture n'est crédité à ce build R1. Le collecteur R2 compile sur
  baseline (1 min 51 s) et candidat (2 min 03 s) ; les 18 tests passent dans
  chaque copie, et leurs 1 203/785 témoins source sont inchangés. Les binaires
  et journaux sont conservés dans `l6-status-binding-diagnostic-r2-d8f9e45cc84f4e8ba3bc12a8556a28d9`.
  La revue R2 trouve encore un P2 : les wrappers publics
  court-circuitent la détection de conflit pendant l'initialisation. Les tests
  directs du mécanisme d'admission ne couvrent pas ce chemin. Une correction R3
  limitée au collecteur et à des tests des trois wrappers est accordée dans de
  nouvelles copies isolées. R3 ferme ce chemin et ses 24 tests passent dans
  chaque copie, après builds release baseline/candidat en 1 min 49 s/2 min 02 s,
  mais sa revue relève un P2 à la fermeture : le dernier échantillon d'I/O est
  pris après la fermeture de l'admission, laissant une fenêtre non signalée.
  La correction R4 figée prend ce dernier échantillon avant le seal atomique
  sous le même mutex, avec huit nouvelles définitions de tests des wrappers
  et de la finalisation réelle (32 au total). Les 15 artefacts et les inventaires
  de 1 203/785 fichiers correspondent aux empreintes ; la nouvelle revue source
  accepte R4 sans constat P0–P3. Les builds release baseline/candidat passent
  en 1 min 54 s/2 min 03 s, puis les 32 tests du collecteur passent dans chaque
  copie. Les sources sont revérifiées après chacune des quatre commandes ; six
  exécutables et les journaux sont conservés dans le
  [manifeste de compilation R4](../../.layrs-local/validation/l6-status-binding-diagnostic-r4-29eb5a388d044b8a80dc4572b74630b8/root-build-review-manifest-r4.json).
  La recette d'exécution 32 fichiers est figée séparément dans
  [son dossier de revue](../../.layrs-local/validation/l6-status-binding-diagnostic-r4-29eb5a388d044b8a80dc4572b74630b8/fixture-recipe-review-r4.md).
  ROOT vérifie les deux empreintes du gel et les 12 artefacts de compilation.
  La revue critique accepte la recette sans constat P0–P3. La SPEC suspend
  l'exécution pour un P2 : les audits comparent les objets observés entre eux,
  sans imposer les 69 objets initiaux attendus ni l'absence de pins. La nouvelle
  [recette R2](../../.layrs-local/validation/l6-status-binding-diagnostic-r4-29eb5a388d044b8a80dc4572b74630b8/fixture-recipe-review-r4-r2.md)
  fixe les 32 cardinalités initiales à partir des sources figées, dont 69 objets,
  36 arêtes, sept racines GC et zéro pin. ROOT vérifie ses empreintes, le diff
  limité et la syntaxe Node. La revue indépendante R2 clôt le P2 sans nouveau
  constat P0–P3 ; elle confirme les cardinalités dans les deux cohortes et la
  conservation byte-exacte du protocole couvert par la revue critique R1.
  ROOT exécute ensuite les huit commandes sous créneau exclusif confirmé :
  elles passent toutes, sans relance, en 29,828 secondes sur le plafond de 900.
  Les deux audits de chaque cohorte et les témoins de payload passent ; les
  69 objets indexés et physiques restent tous atteignables depuis les racines
  de rétention. Les 32 fichiers/16 384 octets et leurs identités sont conservés.
  Les sources et les pins sont revérifiés.
  Les [preuves natives TlGfms](../../.layrs-local/validation/l6-status-binding-diagnostic-r4-29eb5a388d044b8a80dc4572b74630b8/fixture-recipe-run-TlGfms/runtime-review-manifest.json)
  sont acceptées par la revue indépendante, sans constat P0–P3 : les 33 pins
  d'entrée, 34 artefacts runtime et deux inventaires source concordent, ainsi
  que les sorties brutes et la comptabilité. Le créneau exclusif reste attesté
  par ROOT, sans preuve d'inactivité de toute la machine. Dans cet échantillon,
  les intervalles internes
  de vérification des connexions portent 328 126 des 379 345 opérations d'E/S
  supplémentaires observées, soit 86,50 %. CAS et scanner gardent le même travail
  compté (107 lectures CAS/66 135 octets ; deux passes/64 visites de fichiers).
  Cette attribution par intervalles reste une observation instrumentée, avec
  caches non purgés et surcoût de mesure ; elle ne prouve ni un coût par syscall
  ni une causalité de performance généralisable. Une proposition source est
  demandée pour partager le parcours des ancêtres au sein de chaque contrôle
  Store/métadonnées existant. Elle révèle un contre-exemple : après validation
  de la racine, son remplacement peut laisser le parcours relatif réussir sur
  l'ancien handle, alors que le second parcours absolu refuse le nouveau chemin.
  Les handles existants ne garantissent pas l'attachement au nom ; la faisabilité
  du remplacement avec tous les handles Store reste à qualifier sous Windows.
  ROOT maintient les deux observations et suspend l'optimisation. Seul un lot
  de tests déterministes est accordé dans une nouvelle copie isolée de R7, avec
  hooks `cfg(test)`, fixtures conservées et aucun changement live ou release.
  Le [gel de tests de remplacement R1](../../.layrs-local/validation/l6-binding-namespace-race-r1-35e1c4088b0e444bbc4cd5d9b9271b72/freeze.json)
  ajoute six cibles ignorées. La compilation ROOT passe en 1 min 12 s, avec
  786 fichiers source inchangés ; aucun test n'est encore exécuté. L'exécution
  reste retenue pour corriger le lecteur témoin de la base : il demande DELETE
  via `PinnedDirectory::open_file`, incompatible avec le partage READ/WRITE du
  handle SQLite conservé. Ce constat vient des sources, pas d'un échec natif
  observé. La correction doit rester test-only et préserver tous les handles
  Store originaux ; les revues SPEC/critique précéderont les recettes natives.
  Une copie isolée R2 est accordée pour remplacer les seules lectures témoins
  DB par un lecteur sans accès DELETE, acquis avant la coupure et relu via le
  même handle après celle-ci. Les deux nouveaux modules de tests sont les seuls
  fichiers modifiables ; R1, les chemins de production et l'optimisation restent
  intacts. Cette preuve ne sera pas une nouvelle lecture relative de la DB.
  Le gel R2 est vérifié : deux modules changés, 784 fichiers R1 identiques,
  reconstruction des 784 fichiers R7 exacte. La compilation release ROOT
  termine à 0 (1 min 14 s Cargo ; 93,03 s pour l'enveloppe de lancement),
  avec 786 témoins source inchangés. Le binaire et les logs figurent dans le
  [manifeste de compilation R2](../../.layrs-local/validation/l6-binding-namespace-race-r2-f17ed0c39d7f4e4a8af5aa7b13492234/root-compile-review-manifest.json).
  La revue critique accepte la structure sans constat P0–P3. SPEC bloque
  l'exécution sur deux P2 de classification : une validation sémantique avant
  coupure est étiquetée UNQUALIFIED, tandis qu'une indisponibilité I/O durant
  l'initialisation est étiquetée FAILED. Aucun faux PASS n'est constaté.
  La correction est accordée dans une nouvelle copie R3, limitée aux deux
  modules de tests et aux causes typées disponibles ; le scénario et ses
  assertions restent inchangés. Aucune des six cibles natives n'est exécutée
  et aucun parent de fixture R2 n'est créé.
  Le gel R3 conserve 784 autres sources et les inverses R3→R2→R1→R7.
  ROOT compile le binaire release (exit 0 ; 1 min 12 s Cargo, 89,97 s de
  lancement) puis exécute une seule fois chacun des six nouveaux tests purs :
  six PASS, avec les 786 sources et les empreintes du binaire inchangées.
  Le [manifeste de compilation et de tests purs R3](../../.layrs-local/validation/l6-binding-namespace-race-r3-c40428c671da4d9a958c218189abeecc/root-compile-review-manifest.json)
  conserve aussi deux erreurs du lanceur ROOT : un contrôle du nombre
  d'arguments refuse avant tout lancement, puis un contrôle de sortie contiguë
  interprète mal les messages `--nocapture` du troisième test. Son exit 0 et
  son bilan exact d'un test réussi sont vérifiés sans réexécution ; les deux
  paniques synthétiques attendues sont interceptées par ce test.
  La revue SPEC R3 ferme les deux sites P2 initiaux, mais maintient un P2 :
  `read_bounded` déballe encore les erreurs typées du témoin non-DB avant
  coupure, sans classification ni phase structurée. Un refus de partage natif
  conservé devient ainsi FAILED au lieu d'UNQUALIFIED. Aucun faux PASS n'est
  trouvé. La revue critique accepte la structure, les durées de vie et les
  deux corrections initiales sans constat P0–P3, sous réserve d'accord SPEC.
  ROOT confirme le P2 restant. L'inventaire en lecture seule recense les
  lectures, probes d'identité et requêtes selon leur stade, ainsi que les
  causes effacées qui doivent rester FAILED. Une correction R4 est accordée
  dans les deux seuls modules de tests d'une nouvelle copie isolée : contexte
  explicite, mêmes opérations ordonnées, mêmes bornes et durées de vie,
  tests purs ciblés sur les wrappers réellement utilisés. Les preuves R3 et
  le lecteur DB retenu restent figés. Les six recettes natives et la création
  du parent restent suspendues ; aucune qualification de course ni autorisation
  d'optimisation n'en découle.
  Le gel de course R4 corrige les wrappers de témoins et requêtes dans les
  deux modules autorisés. Les 784 autres sources, le lecteur DB retenu et les
  six tests purs antérieurs restent identiques. ROOT vérifie les inverses
  jusqu'aux 784 fichiers R7 et les preuves protégées R1–R3, compile
  en release (exit 0 ; 1 min 18 s Cargo, 94,92 s de lancement), puis exécute
  les dix cibles pures exactes une seule fois : dix PASS, 222 tests filtrés
  par cible. Les paniques synthétiques attendues sont interceptées ; sources
  et binaire restent inchangés. Le
  [manifeste ROOT R4](../../.layrs-local/validation/l6-binding-namespace-race-r4-a36639e706e14486bf3eaa4a118aea7a/root-compile-review-manifest.json)
  lie 23 artefacts de gel et 43 preuves ROOT. Les deux revues SPEC et critique
  R4 acceptent le gel sans constat P0–P3 et ferment le P2 des témoins.
  ROOT libère ensuite six processus natifs exacts et sériels, avec un parent
  jetable neuf et le helper borné existant. Les deux contrôles sains passent
  (directory et cinq modes du vrai Store). Le troisième, directory root,
  termine UNQUALIFIED avec exit 101 et raw 5 PermissionDenied dans l'appel
  composé de déplacement (open/rename/sync/postcheck). La file s'arrête
  immédiatement : Store root et les deux ancêtres restent non exécutés.
  Ce journal ne distingue pas le sous-appel en cause et ne prouve pas qu'un
  déplacement physique n'a pas précédé le refus. Les 23
  [artefacts natifs R4](../../.layrs-local/validation/l6-binding-namespace-race-r4-a36639e706e14486bf3eaa4a118aea7a/native-run-850369366add47b8beb2410ababcb827/runtime-review-manifest.json)
  conservent les trois commandes, sorties, mesures et observations ; aucune
  réexécution, suppression, modification du partage ou des privilèges.
  Sources, binaire et helper restent inchangés. La revue indépendante
  accepte les preuves comme file arrêtée et non qualifiée, sans nouveau
  constat P0–P3 ni autorisation de poursuivre. Elle confirme les deux contrôles
  sains, les trois processus distincts, leurs sorties et mesures complètes ;
  les contrôles sains ne testent ni le lecteur DB de la course ni une mutation
  avec tous les handles Store. L'analyse source exclut les deux synchronisations comme origine
  du raw 5 propagé : leur PermissionDenied Windows est converti en succès.
  Les ouvertures, revalidations, renommage et certains postcontrôles restent
  des causes possibles ; aucun défaut intrinsèque de la primitive n'est établi.
  ROOT accorde un R5 source uniquement dans une nouvelle copie isolée :
  marqueurs de phase/completion cfg(test) dans le vrai directory_move et
  un relevé structuré au retour du seul appel existant. Les deux fichiers
  autorisés sont directory_move.rs et le module de tests secure_fs ; le
  module Store, les appels natifs, droits, partages, durées de vie et dix
  tests purs R4 restent inchangés. Deux contrôles purs au maximum pourront
  vérifier cet état borné en mémoire. Le gel R5 conserve exactement
  784 sources sur 786 et les dix tests purs existants. ROOT vérifie
  l'inverse jusqu'à R7, puis compile en release avec exit 0 (1 min 17 s
  Cargo ; 95 045,6068 ms au lancement). L'inventaire compte 225 tests ;
  aucun corps de test R5 n'est exécuté à ce stade. Le
  [manifeste ROOT R5](../../.layrs-local/validation/l6-binding-namespace-race-r5-6c96f7942da3463c9f708bf8f0fe5af7/root-compile-review-manifest.json)
  lie le binaire retenu, 25 artefacts de gel et dix artefacts ROOT.
  Les nouvelles revues SPEC/critique approuvent le gel sans constat P0–P3.
  Les deux nouveaux tests purs passent chacun une fois ; les dix anciens
  ne sont pas relancés. ROOT libère ensuite la file bornée sur un parent
  neuf : deux contrôles sains passent, puis directory-root reste
  UNQUALIFIED/raw5, exit 101. La trace valide localise désormais le retour
  dans le helper Rename : masque entré 1023, masque terminé 511 ; aucune
  synchronisation ni phase suivante n'est atteinte. Le retour ne prouve
  toujours ni l'état physique final ni la cause Windows sous-jacente.
  Les trois cas suivants restent non exécutés, sans retry ni nettoyage.
  Les [preuves natives R5](../../.layrs-local/validation/l6-binding-namespace-race-r5-6c96f7942da3463c9f708bf8f0fe5af7/native-run-e5b517d5b98341d6a28f477f53395031/runtime-review-manifest.json)
  conservent les sorties, mesures, observations et la disposition bornée.
  La revue runtime indépendante accepte la file arrêtée sans constat
  P0–P3 et confirme les masques, le retour d'erreur du helper Rename et
  l'absence de phase suivante. La cause Windows et l'état physique final
  restent inconnus. L'analyse source situe le raw 5 dans la conversion du
  statut négatif de NtSetInformationFile(FILE_RENAME_INFORMATION) ; les
  refus synthétiques antérieurs n'ont pas de code OS et la collision
  explicitement traitée retourne Ok(true). Le code de production acquiert
  lui aussi les parents en traversée : aucune faute de montage du test ni
  correction de droits n'est établie. Le R6 test-only conserve le NTSTATUS
  déjà retourné, sans nouvel appel ni changement de permissions, partage
  ou durées de vie. Ses trois fichiers autorisés changent ; les 783 autres
  restent identiques. ROOT vérifie le gel et la reconstruction exacte,
  compile en release avec exit 0 (1 min 15 s Cargo ; 95 986,2828 ms au
  lancement), puis retient l'exécutable choisi par le JSON Cargo.
  Le [manifeste ROOT R6](../../.layrs-local/validation/l6-binding-namespace-race-r6-63a11402c8994c31affbce8fde4b425e/root-compile-review-manifest.json)
  lie 29 artefacts de gel et 11 de compilation ; l'inventaire retrouve
  226 tests et les 19 définitions concernées. Les nouvelles revues
  SPEC/critique acceptent la source sans constat P0–P3 ; les trois contrôles
  purs de trace passent chacun une fois, avec 225 tests filtrés. Les
  unwinds synthétiques attendus restent dans les sorties brutes.
  ROOT commet ensuite un écart d'ordre de lancement : l'inventaire est
  utilisé directement au lieu de la séquence approuvée. Le contrôle
  directory passe, puis directory-root s'arrête UNQUALIFIED/101 avant
  l'exécution du contrôle Store. Ce n'est pas le préfixe approuvé : quatre
  cibles restent non exécutées, sans reprise ni substitution par R5.
  Les [preuves natives R6](../../.layrs-local/validation/l6-binding-namespace-race-r6-63a11402c8994c31affbce8fde4b425e/native-run-ce27561af0d0456b9adca8dc08639f76/runtime-review-manifest.json)
  conservent le plan erroné, le lanceur, les résultats et un addendum
  d'incident. La trace valide capture C0000022, STATUS_ACCESS_DENIED,
  au retour de NtSetInformationFile, puis le raw 5 original. Elle n'établit
  ni la capacité refusée ni l'état physique final. La revue indépendante
  confirme un P2 d'orchestration ROOT : la file est non conforme, tandis
  que ses observations bornées restent exploitables. Aucun autre constat
  P0–P3 n'est relevé dans cette revue. Toute future file devra résoudre
  les six noms approuvés et vérifier leur ordre complet et leurs arguments
  avant le premier lancement ; l'addendum ne répare pas ce run.
  ROOT clôt la série d'observateurs répétés ; aucune comparaison de droits
  ni nouvelle exécution n'est accordée. Les traces R4/R5 restent intactes.
  Aucune course n'est qualifiée ; l'optimisation reste suspendue.
  La recette R1 et les échecs précédents restent conservés.
- L7 : les deux recettes complètes se terminent avec succès : clients HTTPS et
  runner Docker (1 test, 1 347,76 s), puis Router/PostgreSQL et interférences
  Layer/Marker (1 test, 87,52 s). Les deux lignes PASS du runner réel sont présentes,
  sans variable de raccourci. Les approbations avec verdicts de fixture et le CI
  réel à zéro approbation restent des scénarios distincts. Voir
  [les preuves L7](worktrees-partial-vfs-reviews.md); la revue finale R2 est PASS
  sur ce périmètre, avec les limites de provenance des deux binaires explicites.
- Desktop : les sept empreintes de la correction de réponse R4 sont inchangées.
  La reprise complète passe **87 tests navigateur** (`desktop-r5-full-browser-fixed.log`)
  et le build de production réussit (`desktop-r5-production-build.log`). Le premier
  lancement, conservé dans `desktop-r5-full-browser.log`, a échoué faute de navigateur
  au chemin Playwright par défaut. La reprise emploie le Chromium déjà installé.
  Ces tests simulent les commandes natives. Les neuf tests Rust du pont Git
  Tauri passent ensuite (`desktop-tauri-git-r2.log`), après réconciliation additive
  du verrou Desktop et téléchargement des versions exactes. Un WebView2 réel
  utilisant le protocole Tauri embarqué crée deux Worktrees, conserve leurs
  captures et octets distincts, puis révèle un refus erroné du snapshot Git dû
  à l'ordre des clés JSON. La destination reste absente et la preuve d'échec est
  conservée dans `desktop-native-r2-bf3ff7a5445e4e96828777615089c21b`.
  Le correctif frontend passe sept tests de modèle, neuf tests navigateur,
  TypeScript et le build; sa revue indépendante ne relève aucun blocage.
  La reprise dans un nouveau WebView natif réussit : dépôt nu, un commit sans
  parent, une référence et 49 octets exactement conservés. Git fsck et l'audit
  Store sont propres et complets. Voir le
  [rapport natif R3](../../.layrs-local/validation/desktop-native-git-r3-76330e052b294464a53e031656e7d628/report.md).
  Les parcours UI distants et partiels restent hors de cette preuve ciblée.

## Limits

- Le programme ne remplace pas les snapshots par des changesets, ne retire pas
  Layers/Markers/Releases et ne rend pas Layrs dépendant d'un serveur en ligne.
- Pas de caches régionaux, Spanner, ordonnanceur CI hébergé ou validation seulement
  après intégration. Les pistes conditionnelles nécessitent leur déclencheur.
- Aucun succès historique du dépôt ne prouve les changements de cette branche.
- Les primitives OS disponibles, les plateformes de test et les données réellement
  préparées bornent les garanties annoncées. Ne pas masquer une limite par un mock.
