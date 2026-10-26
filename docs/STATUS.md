# État courant de Layrs

## Copie d’historique importé

La copie SQL vérifie les collisions avant écriture, conserve exactement les douze
ensembles du préfixe et accepte un rejeu identique. Les contrôles FK restent dans
le périmètre copié. **Trois recettes SQL passent**, dont la copie après deux
imports ; le lecteur du journal source est qualifié séparément de son writer.
Le [checkpoint d’import](plans/worktrees-partial-vfs-import-seams-checkpoint.md)
conserve les binaires et limites. La création physique et la publication de la
destination restent à raccorder.

## Fermeture SQL d’import et journal de transition

Le lecteur vérifie maintenant le préfixe immuable complet, les origines après
plusieurs imports, leurs frontières et les maps d’activation historiques. Le
journal lie son reçu au plan complet et aux identités physiques obtenues après
création. La compilation workspace, **117 tests purs** et **trois recettes SQL**
passent. Le [checkpoint d’import](plans/worktrees-partial-vfs-import-seams-checkpoint.md)
conserve les preuves et leurs limites. La création de destination gérée et son
writer reprenable restent l’étape en cours.

## Reprise de capture après arrêt brutal

Deux recettes natives arrêtent réellement le processus de capture après Staging,
puis après Accepted avant sa réponse. Un autre processus reprend avec le binding
A/P fixé avant l’arrêt. Les deux passent : une capture, un reçu, une protection,
quota libéré et octets du checkout conservés. Le
[checkpoint de capture](plans/worktrees-partial-vfs-capture-checkpoint.md)
conserve le binaire et les limites ; le cut intermédiaire installation/protection,
les fermetures incertaines et la coupure électrique restent non qualifiés.

## Lecture d’historique importé et statut provider

La lecture des captures et publications peut vérifier une origine héritée sans
récursion dans la provenance. La commande dédiée
`layrs partial provider status --ticket FILE` est câblée avec un ticket borné
lu sans suivre de liens. Elle reste conditionnée à la déclaration explicite du
profil SQLite d’un hôte audité ; le CLI générique ne s’accorde pas ce profil.

Les **110 tests purs provider**, les **deux tests ciblés client/CLI** et les
**cinq recettes de métadonnées, ticket et régressions ordinaires** passent.
Le [checkpoint des lecteurs](plans/worktrees-partial-vfs-import-seams-checkpoint.md)
conserve leurs limites. Le validateur SQL complet d’import et les transitions
managed/companion restent en cours ; cette tranche ne les déclare pas terminés.

## Export de récupération du 26 octobre 2026

La session Active peut exporter les fichiers vérifiés vers un dossier ordinaire
distinct. Le manifeste distingue les fichiers candidats au travail géré, les
modifications à récupérer séparément, les suppressions et les octets manquants.
Une lacune produit un résultat explicitement incomplet. L’annulation avant la
publication finale conserve les fichiers sans annoncer une réussite.

**109 tests purs provider, 45 tests purs VFS et cinq recettes natives ciblées
passent**, ainsi que le test d’annulation entre blocs de vérification. Le
[checkpoint d’export](plans/worktrees-partial-vfs-recovery-export-checkpoint.md)
conserve le binaire, les résultats et leurs limites. Trois recettes natives
supplémentaires couvrent remplacement, suppression et refus de modification
protégée dans le [checkpoint de capture](plans/worktrees-partial-vfs-capture-checkpoint.md).

Les codecs purs de provenance et companion sont prêts ; leur lecteur SQL et
les transitions gérées restent en cours. L’expansion de sélection, les essais
de panne restants et l’intégration CLI/Desktop restent à terminer.

## Capture provider du 26 octobre 2026

La session Active capture maintenant une image sélectionnée, conserve un reçu
durable, reprend une tentative Staging identique et permet son annulation
explicite. Les captures acceptées libèrent leur quota réservé tout en gardant
les protections du contenu. Les callbacks locaux restent utilisables pendant
les préfixes de capture valides.

**97 tests purs** et la compilation de tout le workspace passent. Les recettes
natives qualifient séparément verrou interprocessus, SQL, installation,
déduplication, reprise et annulation ; quatre régressions shared ordinary passent.
Le [checkpoint de capture](plans/worktrees-partial-vfs-capture-checkpoint.md)
conserve les résultats par binaire, les deux erreurs de fixtures corrigées et les
limites de qualification. L’export, managed/companion, l’expansion de sélection et
l’intégration CLI/Desktop restent à poursuivre.

## Activation et callbacks locaux du 26 octobre 2026

L’activation écrit maintenant la génération 1 et sa correspondance exacte avec
la source immuable, puis rend le membre familial Ready. Elle conserve les pins
et les effets confirmés après refus. La réouverture Active et la reprise depuis
le disque ne recréent aucun placeholder. Une session distincte sert le contenu
local vérifié ; ses callbacks ne téléchargent rien.

Les binaires conservés r1/r2/r3 passent respectivement **9, 13 et 9 recettes
isolées**. Le dernier comprend deux vrais callbacks Windows avec lecteur externe
et cinq régressions ordinaires ; **83 tests purs** passent sur ce binaire. Les
13 tests SQL d’activation passent dans r1. Le
[checkpoint d’activation](plans/worktrees-partial-vfs-activation-checkpoint.md)
distingue précisément ces preuves et les limites.

La récupération shared privée chaude est désormais refusée sans modification
quand le backend ne peut pas être prouvé en lecture seule. La récupération
familiale avec privé ordinary lisible reste testée. La fixture de récupération
nonshared corrigée passe dans activation r5. La capture provider dispose maintenant
du checkpoint ci-dessus ; les transitions managed/companion, l’expansion
de sélection et l’intégration produit restent à terminer. Le chantier continue.

## Réouverture et lectures locales du 26 octobre 2026

Le détenteur préparé se rouvre maintenant en lecture seule sans recréer les
placeholders ni ressusciter un fichier supprimé. Le lecteur de plages locales
vérifie les identités de contenu et conserve son buffer intact en cas de manque,
corruption ou annulation. Les requêtes SQLite longues et les comparaisons de
descripteurs consultent le signal d'annulation.

Le dernier binaire passe **82 tests purs, 10 recettes SQL et 8 recettes isolées** ;
la compilation du workspace avec toutes ses cibles passe aussi. Les recettes
de réouverture précédentes restent consignées dans le
[checkpoint des lectures locales](plans/worktrees-partial-vfs-local-read-checkpoint.md).
Le codec d'activation est prêt, mais son writer, la capture et l'intégration
CLI/Desktop restent à poursuivre. Aucun hôte produit n'est activé par ce checkpoint.

## Préparation native du 26 octobre 2026

Le validateur SQL post-bootstrap et le writer du ticket sont implémentés.
Le détenteur publié peut désormais créer les placeholders sans source de contenu,
puis conserver une preuve native READY liée au Store. Les refus gardent les
pins, les effets confirmés et les causes originales.

**7 recettes natives, 79 tests purs et 9 recettes SQL passent** sur le binaire
audité. Le [checkpoint de préparation](plans/worktrees-partial-vfs-preparation-checkpoint.md)
consigne les preuves et limites. La reprise durable, les lectures locales,
l'activation, la capture et l'intégration produit restent en cours ; le chantier
continue au-delà de ce checkpoint.

## Contrat de préparation du 26 octobre 2026

Le [contrat post-bootstrap](plans/worktrees-partial-vfs-preparation-contract.md)
est défini, relu et implémenté sous forme de codec pur `mount.v2`. Il lie le
ticket VFS à la source et aux reçus exacts, et distingue l'intention d'enregistrement
de l'état préparé. Il tient dans le schéma 8 existant ; le format bootstrap v1
et son validateur restent inchangés et refusent v2.

**129 tests réussissent** : 77 provider, 45 VFS et 7 sélections SQLite explicites.
La compilation de tout le workspace passe ; la revue indépendante ne relève
aucun blocage dans le périmètre du codec et de ses comparaisons.
La suite est le validateur SQL post-bootstrap complet et le transfert du
détenteur, puis le writer du ticket et la population native. Le codec livré ne
modifie pas les Stores de production et n'active pas le provider.

## Reprise du 26 octobre 2026

La passation distante `10f93d7` est intégrée par `b54d18c`, après la fusion
des travaux locaux `a4c3551`. Les **20 268 fichiers** du
[paquet portable](continuation/worktrees-partial-vfs/README.md) sont restaurés
dans `.layrs-local/validation` ; taille et SHA-256 de chaque fichier ont été
vérifiés. Les preuves historiques n'ont pas été rejouées ni modifiées.

La reprise porte sur le catalogue et les descripteurs de métadonnées empruntés
au holder `ProviderControlPublished`. La revue de conception accepte ce
périmètre, avec une seconde validation exigeant elle aussi `ControlPublished`.
L'implémentation est terminée : **10 recettes natives**, **6 régressions du
validateur** et **73 tests purs** passent ; la compilation de tout le workspace
passe également. Le [checkpoint du catalogue](plans/worktrees-partial-vfs-catalog-checkpoint.md)
consigne le contrat, les preuves, les échecs antérieurs conservés et la suite.
Population native, lecture de contenu, capture et activation CLI/Desktop restent
à réaliser.

## Intégration locale du 25 octobre 2026

Les travaux locaux du début octobre sont réintégrés sur le checkpoint distant
`0eba2e5`, branche `feature/worktrees-partial-vfs` : finition L6, Git Desktop,
organisations et équipes, lenses, authentification des appareils, clonage de
projets vides et inspection des destinations existantes. Les migrations
0020–0023 sont conservées ; les worktrees et le VFS de la branche distante
restent présents.

La fusion conserve les deux parcours Git Desktop : **Pont Git** (`/git`) et
**Import / export Git** (`/git-assistant`). Le second utilise désormais les
contrôles d'état et de destination du pont distant. Les aperçus serveur gardent
leur plafond de 1 Mio avec le nouveau lecteur à cache vérifié. L'inspection des
destinations accepte aussi les locators validés des worktrees secondaires.

Le [checkpoint du 25 octobre](plans/worktrees-partial-vfs-checkpoint.md)
décrit le chantier Worktrees/VFS et les travaux encore ouverts. Les preuves R7
initialement absentes ont été restaurées le 26 octobre (voir ci-dessus). Cette fusion ne clôt pas
ce chantier et ne vaut pas qualification de distribution.

Vérifications de cette intégration : compilation du workspace Rust avec toutes
ses cibles et de Tauri avec ses tests (`cargo check --locked --offline`,
`--all-targets`), TypeScript Desktop/Web/UI, builds Vite Desktop/Web,
**51 tests renderer historiques**, **80 tests renderer Worktrees/Git/hors ligne**,
**44 tests des contrats Desktop**, **9 tests du lanceur local**, contrats SDK et
lenses intégrées, et **23 checksums de migration** réussis. Les fixtures distantes
reconnaissent désormais le catalogue Lens et les contextes/destinations ; les
assertions de restauration locale continuent de refuser les appels réseau de
restauration. Le schéma Lens conserve des fins de ligne LF sous Windows.

La commande `cargo test --locked --offline -p layrs-client-core
-p layrs-git-bridge -p layrs-store-server --lib -- --test-threads=1` réussit :
**109 tests client**, **41 tests Git** et **60 tests serveur**, avec **7 tests
spécialisés ignorés**. Les nouvelles régressions vérifient le locator secondaire
et sa liaison partagée, ainsi que le plafond d'aperçu avant lecture des blocs,
sa borne exacte et le refus d'un bloc devenu indisponible dans le cache.
Les premiers essais sandboxés ont échoué sur les permissions Windows ; les
résultats réussis ci-dessus proviennent de la réexécution avec les accès requis.
Les **2 tests natifs de l'adaptateur Git Desktop** passent également après
correction de la représentation des chemins Windows : l'état attendu utilise
le chemin enregistré du checkout ; les chemins canoniques restent réservés aux
contrôles de chevauchement. La revue critique ciblée ne relève aucun blocage
sur les identités Git, les aperçus autorisés et les locators secondaires.

Les contrôles globaux de formatage et de taille restent en échec sur des fichiers
inchangés du checkpoint distant : respectivement **23 fichiers** et **6 fichiers**.
Clippy strict, réexécuté sur le workspace et toutes ses cibles, échoue sur les
diagnostics du code VFS inchangé ; cet échec était déjà signalé dans le journal
Worktrees/VFS et empêche de qualifier la porte Clippy globale. Les
recettes PostgreSQL/HTTPS complètes et les tests CFAPI ignorés ne sont pas
qualifiés par ces contrôles de fusion.

## État produit et preuves historiques du 6 octobre 2026

La suite de ce document conserve les résultats de la passe locale du 6 octobre,
sans les présenter comme des tests de la fusion. Point de départ de cette passe :
`4601b16`, branche `redesign/foundations`.

## Produit

Le 6 octobre, Desktop distingue les destinations de clonage déjà occupées :
lier le même projet, ouvrir la synchronisation d'un autre projet accessible,
préparer la publication d'un projet local absent du serveur, ou refuser le
clonage lorsque le contenu ou les droits ne le permettent pas. Les fichiers
existants sont conservés. La préparation ne publie rien ; la création sur le
serveur puis l'envoi des captures restent explicites. Le rejeu exact de création
reste possible après une réponse perdue.

La page **Changements** explique désormais que **En cours** compare le dossier
à la capture la plus récente, automatique ou manuelle. Une fois capturés, les
fichiers restent visibles sous **Dernière capture**, même après réouverture ;
le badge **Automatique** identifie la capture automatique. Sans observation
locale, la vue est nommée **Capture de référence**. Les versions nomment les
étapes importantes et la synchronisation porte la publication. Le test Rust
StepDiff et les **28 tests renderer Desktop** passent, dont les réponses
tardives et la distinction avec une capture reçue. La régression HTTPS/PostgreSQL
sur les destinations et le clonage initial passe sur une base jetable.
TypeScript, Clippy ciblé, formatage et builds Web/Desktop passent. L'exécutable
local et le serveur de développement ont été reconstruits ; le nouvel endpoint
HTTPS a été vérifié avec et sans authentification, sans créer de projet réel.
Relancer Desktop pour charger le nouvel exécutable.

Le socle natif comprend captures immuables, restauration, Layers, fusion,
synchronisation V3, permissions, revues, versions et preuves CI. Le pont Git
local existe dans `layrs-git-bridge`. Le serveur et le Web se lancent avec
`npm run dev` ; Desktop utilise Tauri et le cœur Rust commun.

La refonte L0–L5 est intégrée depuis le 30 septembre : catalogue autorisé,
fondations visuelles, navigation Web/Desktop, travail local et accès limité,
collaboration et administration Web. **L6 est intégré et validé localement** :
composants partagés dans Desktop, assistant Git natif, reprise exacte des demandes
de droits et documentation courante. Les limites ci-dessous restent explicites.
Le détail vérifiable est dans le [registre](design/redesign/03-delivery.md).

La création de projet utilise désormais l'entreprise de l'installation par
défaut. Les organisations multiples et les projets personnels sont deux options
serveur indépendantes, désactivées par défaut. Web et Desktop sélectionnent le
contexte et affichent ses projets autorisés. **Administration → Organisations**
permet désormais la bascule à chaud : choix explicite de l'organisation principale,
avertissement de danger et confirmation saisie. Les autres organisations, projets
et permissions sont conservés mais inaccessibles jusqu'à la réactivation du multi.
Les projets personnels restent une option indépendante. Le sélecteur d'organisation
précède « Tous les projets » et la liste des projets ; son dernier choix crée une
organisation. Il disparaît en mode mono. Les équipes d'organisation sont
réutilisables avec des droits propres à chaque projet ; les anciennes équipes
conservent leur portée et leur historique. Voir le [contrat des contextes](contracts/project-contexts.md).
Le repère de chaque projet dans la navigation Web/Desktop est un triangle :
orienté vers le haut au repos, tourné de 90° vers la droite à la sélection,
avec les couleurs précédentes conservées.
Le Layer initial et les lenses sont créés dans la même transaction. La page
**Administration → Lenses** installe des paquets vérifiés sur le serveur ; les
**Réglages → Lenses** épinglent une version par projet. Web et Desktop chargent
les composants clients du projet à la demande. Les aperçus publiés lisent le
texte UTF-8 autorisé jusqu'à 1 Mio. Voir le [guide](guides/projects-and-lenses.md)
et le [contrat](contracts/lens-distribution-v1.md).

Limite actuelle : l'adhésion à une organisation passe par une invitation ou un
accès direct à un premier projet. L'administration des adhésions sans accès à
un projet n'a pas encore d'écran dédié.

## Vérification

Le clonage Desktop/CLI accepte désormais les projets vides créés depuis le Web :
une seule Layer Base sans tête, à la génération zéro, sans historique référencé.
Il conserve la capture initiale vide localement pour permettre les captures et
publications suivantes ; le clonage ne modifie pas le serveur. Les projets déjà
créés bénéficient du correctif sans migration ni recréation. Les droits d'accès,
le refus de cloner dans un dossier occupé et la protection des changements locaux
sont conservés ; les actions explicites ci-dessus permettent d'utiliser un projet existant.
Les 53 tests unitaires du cœur client passent, ainsi que Clippy et la compilation
Desktop. La régression HTTPS/PostgreSQL ciblée passe et la revue critique ne
relève aucun blocage. Elle couvre les lenses, deux clones initiaux, la reprise
du journal, la publication et la récupération des fichiers. Les courses entre
premières publications et les crashes réels ne sont pas injectés dans ce nouveau test.
La recette native générale reste bloquée dans `unchanged_drill.rs` : d'anciens
scénarios provisionnent deux fois le slug `project` dans le même contexte
d'entreprise, alors qu'ils utilisaient auparavant des organisations distinctes.

Le bouton Desktop « Parcourir… » utilise désormais le sélecteur moderne Windows
`IFileOpenDialog` en mode dossier, rattaché à la fenêtre Layrs. Le lancement
PowerShell de l'ancien `FolderBrowserDialog` est supprimé. Le dossier initial
(ou son parent existant), les chemins Unicode et l'annulation sont pris en charge.
`cargo check`, `cargo clippy -D warnings`, le formatage et la compilation Desktop
passent. La vérification visuelle du dialogue reste à effectuer : le service
Computer Use était indisponible. L'exécutable local est mis à jour ; relancer
Desktop pour charger cette version.

Finition du 6 octobre : la page d'autorisation utilise le logo original et les
tokens partagés de Studio, les polices locales Inter/Audiowide/JetBrains Mono,
le bouton corail-orange et la préférence de thème de l'interface. Ses ressources
sont embarquées dans le serveur ; les vues sombre, claire, mobile et confirmation
ont été inspectées sur la recette HTTPS. Les sept tests Web passent.

Le catalogue Desktop de « Nouveau projet → Récupérer un projet de l'équipe »
charge maintenant après connexion même si aucun dossier n'est encore choisi.
Un compte sans organisation accessible reçoit un état vide explicite.
Six vérifications renderer ciblées passent (trois de correction/parcours et
trois de revue indépendante sur l'isolation et les changements de serveur).
La recette Desktop Tauri réelle passe en commençant par cette connexion sans
dossier et en vérifiant l'apparition du projet distant. `pnpm check` passe.

L'autorisation Desktop/CLI par `/auth/device` charge désormais son script à
travers le proxy. La page peut utiliser un compte connecté dans un autre onglet
Studio Web de la même origine, avec choix explicite si plusieurs comptes sont
ouverts et confirmation du compte et de l'appareil avant autorisation. Le pont
inter-onglets ne transmet aucun jeton et ne permet que la vérification et
l'autorisation d'un code, liée à la session et à une vérification de courte durée.
Sans onglet connecté, le formulaire de connexion reste disponible. Recharger
l'onglet Studio après mise à jour pour disposer du pont.
Les quatre nouveaux tests HTTPS réels vérifient le formulaire manuel, le compte
déjà connecté, le choix entre deux comptes, l'annulation et l'invalidation après
changement de session. Un script bloqué laisse le formulaire désactivé.
La suite Web compte désormais **7 tests réussis** ; `pnpm check` passe et la
revue de sécurité indépendante n'a pas de constat bloquant restant. L'instance
locale sert bien `/auth/device.js` en JavaScript et le bundle Web correspondant.

Pour les contextes et la bascule mono/multi du 5 octobre : `pnpm check`
passe toutes les portes statiques, avec **23 migrations** vérifiées. Les tests
PostgreSQL couvrent les quatre combinaisons d'options, la réinitialisation,
la conservation des anciennes organisations et permissions, l'accès personnel
réservé au propriétaire, le catalogue et les équipes partagées entre projets.
Le rollback des changements d'équipe et la révocation à la limite des versions
sont vérifiés. Les **19 tests renderer Desktop** passent, dont trois régressions
sur le changement de serveur et la disponibilité des dossiers locaux. La recette
Desktop Tauri avec serveur HTTPS réel passe également. Les **7 tests Web HTTPS**
passent : contextes et projets personnels, lenses, collaboration et droits.
La recette Web vérifie aussi la confirmation dangereuse, les noms d'organisation
identiques, l'annulation, le refus d'accès direct pendant le mode mono et le
rétablissement. La recette Desktop réelle effectue la même bascule et son retour.
Les tests PostgreSQL `organisation_mode` (1), `native_contexts` (2) et
`native_sync_v3` (1) passent. Un ancien jeton de téléchargement et un reçu de
transaction sont refusés pendant le masquage puis acceptés après réactivation.
Les revues UI et sécurité indépendantes ne conservent pas de défaut bloquant.
Les entrelacements concurrents forcés et la pause/reprise des webhooks ne font
pas l'objet d'une recette dédiée ; leurs contrôles ont été examinés dans le code.
Les neuf tests du lanceur
local passent. Ces résultats ne constituent pas une qualification de distribution.

L'instance locale `https://localhost:19443` utilise le mode multi-organisation
pour conserver `test` et `test2` ; les projets personnels y sont désactivés.
L'API HTTPS authentifiée vérifie les nouveaux réglages, les trois contextes et
leur catalogue filtré. Les trois organisations, les deux projets et les trois
adhésions sont identiques avant/après la mise à jour. Le mode reste multi pour
laisser l'administrateur choisir l'organisation principale dans l'interface.
Le [guide local](development/local-server.md) explique comment
activer indépendamment les projets personnels et conserver les options.

Lors de la qualification L6, avant le chantier lenses du 5 octobre :
`pnpm check` réussi, **27 tests renderer réussis**, recettes réelles
Desktop SCM, équipe, Git, accès limité et Web réussies sur leur environnement
de test. Les constats des revues indépendantes ont été corrigés et revérifiés
dans leur périmètre. Le binaire Desktop Windows avec interface embarquée a été
reconstruit. Les commandes, preuves, empreinte et limites sont dans la
[qualification L6](design/redesign/04-qualification-l6.md).

Les résultats du 30 septembre restent des preuves historiques. Les essais de
cette passe ont été exécutés en plusieurs commandes ; ils ne constituent pas
une annonce de réussite de la suite globale `pnpm test` ou de la CI distante.

Lors de la passe lenses, avant l'ajout des contextes, la recette PostgreSQL du catalogue
valide création dans une organisation partagée, refus d'un membre, reprise
exacte, Layer ouvrable et pins initiaux. La recette Web HTTPS réelle a passé
**2 tests** : installation de la lens Markdown, création organisation/projet,
configuration d'un projet existant, téléchargement et rendu du `README`
publié, en plus du parcours Web de collaboration et de droits. Les **24 tests
renderer sélectionnés**, dont six du runtime des lenses, et le contrat SDK
TypeScript passent localement. La recette Desktop Tauri réelle passe également :
téléchargement et affichage du fichier publié avec Text 1.1.0 dans WebView2,
collaboration et administration, avec sélection du worker embarqué sans variable
de configuration fournie par le test.
Le test serveur ciblé `native_catalog` passe sur PostgreSQL réel avec retrait,
révocation et conservation des références historiques ; cinq tests du
vérificateur de paquets passent avec un module WASM réel. Les 20 migrations et
leurs checksums, ainsi que les contrats TypeScript SDK et paquets intégrés,
passent les contrôles ciblés. `pnpm check` passe aussi toutes les portes
statiques : migrations, taille des fichiers, format Rust, Clippy sur tout le
workspace avec avertissements refusés, TypeScript et builds Web/Desktop.
Le serveur de développement reconstruit répond sur `https://localhost:19443` :
TLS/authentification vérifiés, catalogue des lenses et organisations accessibles.
Les 17 tests Rust du SDK passent aussi. Ces recettes locales ne qualifient pas
une installation sur machine vierge ni un installateur distribué.

Le moteur natif passe 51 tests Weave, quatre tests du worker réel et 53 tests
du cœur client. Les associations explicites, la sélection unique et les
ambiguïtés incluent les lenses réservées à l’affichage. Le contrôle serveur
refuse aussi les nouveaux reçus d’anciens clients dès qu’un projet possède
une configuration, même vide ; les reçus exacts déjà admis restent historiques.
Le test PostgreSQL couvre ce refus après ajout d’une configuration entre
préparation et admission.

Le nouvel exécutable Windows avec interface embarquée se trouve dans
`apps/studio-desktop/src-tauri/target/debug/layrs-studio-desktop.exe`.
Son SHA-256 après reconstruction avec les destinations existantes et l'affichage
des captures du 6 octobre est `6e7845cca86cf24d8adafcf854e529cdcc32756874bbc1e8458c320506f46023`.
Le worker et sa copie dans les ressources sont identiques :
`cd116c202a1f9230d69725faf08b0bb97e6befab5252a30d1f8c2b6fb2e12871`.
Il s’agit d’un build local de qualification, pas d’un installateur signé.

## Suite du projet

La prochaine étape est la qualification de distribution : essai sur machine vierge, pilote
humain encadré et fermeture des critères externes. Pour reprendre
le développement, utiliser ce statut et le registre de livraison ; les anciens
checkpoints sont archivés, pas des tâches encore actives.

## Limites maintenues

- La création du lien distant revalide le dossier après le réseau et refuse
  d'écraser un lien existant. Elle ne protège pas atomiquement contre un processus
  local malveillant substituant le répertoire après l'ultime vérification ;
  cette course adversariale finale n'a pas été injectée dans les tests.

- Le bouton d’intégration des pages de revue reste désactivé : ce raccourci ne
  peut pas lier la publication à la capture et au Layer exacts. Utiliser le
  parcours de synchronisation Desktop en vérifiant ces références ; approuver
  et intégrer restent deux opérations distinctes.
- Aperçus distants limités au texte UTF-8 de 1 Mio ; pas encore de rendu
  binaire des images dans le navigateur de fichiers. Pas d’annuaire général
  lisible par tous les membres.
- Une lens avec composant `analyzer` n'est pas encore installable. Le worker
  JavaScript contrôle délais et tailles d'entrée/sortie, sans quota dur de
  mémoire. Un client hors ligne ne peut apprendre une nouvelle révocation
  qu'à la reconnexion.
- Les règles de fichiers personnalisées ne se combinent pas encore avec la
  gestion par équipes/rôles ; le changement de mode reste explicite.
- Projections partielles : fusion multi-têtes/multi-bases, transformations
  structurelles et restauration de chemins historiques inconnus restent ouvertes.
- Distribution : pilote humain, installation sur machine vierge, qualifications
  externes et budgets de performance restent à établir. Aucun MSI signé,
  déploiement ou résultat de CI distante n’est déduit d’un test local.

Les [neuf qualifications ouvertes](plans/pme/REMAINING.md) concernent le socle
et la distribution, indépendamment de la finition de l’interface. Les anciens
paquets `qualified6/7` précèdent la refonte et ne représentent pas ces sources.
