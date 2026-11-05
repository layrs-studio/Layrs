# Exécution, dépendances et qualification

Plan préparé le 24 septembre 2026. Ce document sert de registre de reprise.
Les cases cochées indiquent des livrables réels, pas une intention.
Actualisation du 5 octobre 2026 : L0–L6 intégrés dans leur périmètre documenté ; L6 validé localement. Le parcours création de projet et distribution de lenses a ensuite été ajouté ; ses preuves sont distinctes de la qualification L6. Voir aussi
[l’état courant du projet](../../STATUS.md).

## 1. Surfaces actives

- Web : `apps/studio-web/src/main.tsx` monte `app/WebApp.tsx`.
- Desktop : `apps/studio-desktop/src/main.tsx` monte `app/DesktopApp.tsx` ;
  `app/ProjectFrame.tsx` route les projets complets et limités. Les anciens
  `NativeWorkbench`, `NativePartialWorkbench` et `NativeRemotePanel` sont retirés.
- Composants partagés : `packages/ui/src/collab`, `team`, shell et primitives.
  Les contrats et composants `native-*` encore référencés ne sont pas tous
  supprimables : certains servent aux accès limités et aux tests spécialisés.
- Contrats de commandes/transports, garde de génération, brouillons et exclusions
  concurrentes sont à préserver pendant extraction en vues.
- Les fichiers publiés ont un aperçu distant autorisé du texte UTF-8 jusqu'à
  1 Mio. Image Lens est au catalogue, mais les octets d'image ne sont pas encore
  rendus dans cet aperçu ; l'interface présente un repli explicite.

La présence d’une route ou d’un composant ne remplace pas la recette du parcours.

## 2. Registre des contrats indispensables

Contrats revus le 5 octobre 2026. Les limites ci-dessous restent distinctes de la recette des interfaces.

| Besoin UX | État actuel | Garde-fou / reste |
| --- | --- | --- |
| Projets par nom | Catalogue V3 paginé autorisé et filtré par contexte, raccordé Web/Desktop | Curseur lié au compte et au contexte ; capacités du projet pour résoudre son contexte sur lien direct |
| Créer un projet serveur | Entreprise par défaut ; organisations et personnel activables indépendamment ; Layer vide et pins initiaux atomiques | Propriétaire du projet distinct de l'administration de l'organisation ; réessai exact idempotent |
| Équipes d'organisation | Annuaire partagé et affectation de rôle par projet | Membres limités à l'organisation ; changements atomiques dans les projets liés ; anciennes équipes conservées |
| Connaître la personne | whoami fournit identité et instance_administrator | Ne pas déduire les droits du kind ; administration indépendante des droits du projet |
| Actions autorisées | Réponse de capacités L0 dérivée des prédicats serveur | Indices UI ; contrôle serveur conservé à chaque mutation |
| Inviter une personne | Sélection par nom dans les comptes de l’instance (admin) ou membres de l’organisation | Pas d’annuaire général de projet ; référence technique en repli |
| Mot de passe | Formulaire Web raccordé ; invalidation des jetons et reconnexion | Pas d’édition de nom/avatar ; Desktop gère la connexion appareil et les jetons |
| Invitations et activité | Inbox globale d’invitations et notifications du projet | Pas d’agrégation globale des notifications |
| Aperçus distants | Texte UTF-8 publié via chemin exact autorisé et viewer de lens isolé | 1 Mio ; image binaire et formats non textuels en repli ; droits revérifiés avant réponse |
| Import/export Git Desktop | Commande Tauri structurée et assistant L6, via le pont existant | Aperçu lié à Operation/View ; export bare vers dossier neuf ; reprise CLI, recette L6 ci-dessous |
| Thèmes et polices | Sombre/clair/système, préférence précoce, Audiowide et polices locales | Contraste exhaustif et autres plateformes non qualifiés par une capture |

Les contrats L0 ont été livrés sans nouvelle migration. Les lenses utilisent
0020, les contextes 0021, les équipes d'organisation 0022 et la bascule de mode
0023. La sélection de
personnes reste bornée aux annuaires autorisés ; les identifiants avancés ne
sont pas masqués par des noms inventés.

Pour chaque complément : documenter contrat, limites, erreurs, pagination, droits,
invalidation et versionnement. Pas de liste reconstruite en scannant des IDs ni
d'API qui expose des projets/identités hors du périmètre autorisé. Les capacités
affichées sont une aide d'interface et peuvent devenir périmées entre deux requêtes.

## 3. Lots et livrables

### L0 — Couverture métier et contrats de navigation

- Reproduire les entrées actives et inventorier les fonctions anciennes encore
  utiles ; établir la matrice ancien contrôle → nouvelle page/action.
- Fixer les routes UI, identités de contexte et modèle des capacités.
- Implémenter catalogue autorisé et les données de sélection manquantes avec
  transports Web et Desktop cohérents ; couvrir équipe, révocation et pagination.
- Décider précisément la frontière du cache de récents et de la préférence de thème.
- Livrable : contrats documentés, tests serveur ciblés et exemple de réponses sans
  données réelles sensibles. Dépendance des pages Projets/Équipe.

### L1 — Fondations visuelles et shell

- Intégrer le vrai logo, Audiowide et police de lecture avec licences locales.
- Introduire tokens sombre/clair, thème précoce et préférence de mouvement.
- Construire primitives accessibles, shell, navigation, état vide, erreur,
  activité, panneaux et palette contextualisée.
- Définir les composants sur au moins connexion, liste de projets, diff et
  formulaire de permissions ; comparer au visuel approuvé.
- Livrable : composants partagés éprouvés, pages de démonstration internes avec
  états clairement fictifs ; aucune donnée fictive dans les routes produit.

L0 et L1 peuvent avancer en parallèle après accord sur interfaces de contexte ;
le shell peut utiliser des fixtures dans ses tests, pas un faux catalogue en production.

### L2 — Entrée, connexion et navigation Web/Desktop

- P01–P04 et P18–P20 ; ajouter changement de mot de passe et autorisation appareil.
- Pages Projets alimentées par L0, dossiers récents Desktop, création guidée.
- Routeur unique par application, liens directs, retour arrière et liens de
  notification ; traitement des anciennes routes sans perte des fonctions.
- Migration des préférences non sensibles ; ne pas déplacer secrets et stores
  pour le seul besoin de changer une page.
- Livrable : une personne arrive réellement dans son projet par son nom.

### L3 — Travail Desktop

- P05, P08–P11 et P23 : changements, capture, Layer, opérations, comparaison,
  restauration, fusion, synchronisation et reprise guidée.
- Intégrer les projets complets et limités dans le même shell avec modèles de
  capacités distincts, sans forcer une commande full-project sur une projection.
- Préserver capture automatique, previews, choix de conflit persistants et
  protections contre l'état devenu obsolète.
- Livrable : modification réelle → capture → restauration/reprise, avec tests
  partiels et concurrence locale/distant.

### L4 — Consultation et collaboration partagées

- P06–P07, P12–P14 : fichiers, historique, revues, validations et versions.
- Extraire les composants existants en sous-vues sans perdre pagination,
  correction de commentaire, versions exactes et brouillons.
- Raccorder aperçus d'images supportées via transport autorisé, prévoir fallback
  non textuel pour les autres formats. Pas de lecteur universel de fichiers.
- Livrable : parcours capture publiée → revue → décision → intégration, identique
  conceptuellement dans Web et Desktop.

### L5 — Équipe, accès et administration

- P15–P17 et P21 : navigation par sujet, sélecteurs lisibles, aperçu des règles,
  gestion des invitations/équipes et opérations de comptes/services.
- Maintenir identité, portée, droits effectifs, délais/expiration et confirmations
  sur rotations/révocations. Révélation des secrets limitée à leur création.
- Livrable : responsable autonome pour inviter et limiter un fichier ; membre
  normal capable de comprendre ses accès sans voir l'administration d'instance.

#### Correspondance de l’ancienne surface d’administration (L5)

Web utilise `packages/ui/src/team/`. L6 raccorde ces mêmes composants au transport
Tauri de Desktop ; la recette native vérifie la couverture de l’ancienne surface.

| Ancien contrôle (`native-admin`, `-invitations`, `-teams`, `-policy-*`, `-storage`, `-metrics`) | Nouvelle page · action |
| --- | --- |
| Mes invitations reçues, accepter/refuser | Boîte de réception (`/inbox`) › Invitations |
| Invitations envoyées, suivantes, révoquer | Équipe et accès (`/projects/:p/team/invitations`) › liste paginée, révocation confirmée |
| Inviter un compte (`principal:…` saisi) | Invitations › Inviter une personne : choix par nom (comptes de l’instance pour l’administrateur d’instance, sinon membres de l’organisation) ; identifiant technique en repli « La personne n’est pas dans la liste » |
| Équipes : charger, créer, modifier, archiver (motif), historique paginé | Équipe et accès › Équipes (dialogues, version attendue, clé d’idempotence conservée en cas de réessai) |
| Accès au projet : rôle d’un `principal:…` (`grantMember`), retrait | Équipe et accès › Membres › Changer le rôle ; Accorder un accès direct (comptes et services) |
| Gestion des accès (mode géré/personnalisé, références) | Réglages › Général (mode) et Avancé (références copiables) |
| Changer la politique : par référence, retour aux équipes et rôles, règles JSON (ajout/remplacement), aperçu puis application | Réglages (`/projects/:p/settings/advanced`) › Avancé, aperçu lié au serveur puis confirmation |
| Droits d’un membre sur un fichier ou dossier | Accès aux fichiers (`/projects/:p/access`) : personne, arbre, droits effectifs expliqués, aperçu avant/après, confirmation |
| Protection des Layers (`layer:…` saisi, approbations, auto-approbation, conflits, recette CI, exécuteurs, âge de preuve) | Réglages › Protections (Layer par nom, garde « non enregistré ») ; vue d’ensemble dans Automatisations |
| Webhooks : créer (secret), lister, livraisons, relancer, renouveler le secret, désactiver | Réglages › Webhooks |
| Notifications du projet : actualiser, suite, marquer lues, ouvrir la revue exacte | Boîte de réception › Activité du projet (projet choisi par nom ; lecture enregistrée après réponse du serveur) |
| Comptes : charger, révoquer, créer (mot de passe initial effacé après 60 s) | Administration (`/admin/accounts`) › Comptes |
| Compte de service : créer avec jeton, renouveler le jeton | Administration › Comptes de service (+ révocation) |
| Stockage de l’instance | Administration › Stockage |
| Activité du serveur (métriques) | Administration › État du serveur |
| Mes appareils et jetons : lister, révoquer | Compte (`/settings/account`) › Sessions et appareils (session courante identifiée) |
| Révélation unique d’un secret (60 s, copie) | Dialogue `SecretReveal` : masqué par défaut, 60 s, jamais réaffiché |

### L6 — Pont Git, intégration et finition

- P22 : adaptateur natif et assistant, traitement des chemins réservés et rapport.
- Vérifier cohérence entre tous les lots, interactions, libellés, thèmes, tailles
  de fenêtre et animations. Supprimer les anciens composants devenus inutilisés
  seulement après preuve de couverture, pas au début.
- Ajouter captures de recette, guide utilisateur et mapping des anciennes routes.
- Reconstruire le Web local et l'application de test pour constater le résultat.
- Livrable : parcours réels vérifiés, journal et commits, limites honnêtement listées.

## 4. Parallélisation et responsabilité des fichiers

| Flux | Propriétaire de travail | Exclusivité / dépendances |
| --- | --- | --- |
| L0 | Backend/contrats | Routeurs et stockage V3, modèles client ; aucun reformatage global |
| L1 | Design system | `packages/ui` fondations/tokens/shell ; contrat partagé fixé avant autres imports |
| L2 | Intégration/navigation | `main.tsx`, routeur, session et pages d'entrée des deux applications |
| L3 | Desktop local | `app/workspace`, `app/projection`, `pages`, `merge`, `sync` et styles propres |
| L4 | Collaboration | Modules communs revue/historique/fichiers/versions dans `packages/ui` |
| L5 | Administration | Modules communs admin/invitations/équipes/policy dans `packages/ui` |
| L6 Git | Pont natif | Adaptateur Tauri/bridge ; aucune modification concurrente de transport sans coordination |

Après L0/L1, L3/L4/L5 peuvent avancer en parallèle avec contrats gelés et fichiers
distincts. L2 fournit le squelette de routes et intègre leurs exports. Un seul
intégrateur modifie `packages/ui/src/index.ts`, le shell et les fichiers d'entrée
à un instant donné. Les modifications de tests partagés passent aussi par lui.
Revue indépendante des contrats d'accès et validation intégrée à la fin.

## 5. Ordre des commits et reprise

Un commit par changement cohérent : contrat catalogue ; fondations/thèmes ;
navigation/entrée ; capture locale ; collaboration ; accès/admin ; Git ; recette.
Fractionner si un lot produit plusieurs comportements vérifiables, pas pour
multiplier les commits. Chaque commit inclut documentation de ses limites et
tests pertinents. Pas de fichiers de travail destinés au produit laissés flottants.

À chaque arrêt : remplir tableau de suivi ci-dessous avec fichiers, commit, tests
exécutés, échecs, tâche suivante et état des services. Ne pas transformer une
case « implémenté » en « vérifié » sans preuve du parcours réel.

| Lot | Implémentation | Qualification | Reprise / preuve |
| --- | --- | --- | --- |
| Direction visuelle | Validée | Accord utilisateur | Maquette graphite, corail/ambre, logo original |
| Plan pages/UX | Rédigé | Revue indépendante ; corrections intégrées | 24 fiches, liens vérifiés ; identité locale, provenance et limites Web explicitées |
| L0 Contrats | Implémenté (catalogue, capacités, whoami, transport Desktop) : `GET /v3/spaces` paginé par curseur scellé lié au principal, `GET /v3/spaces/:space/capabilities` dérivé des prédicats partagés avec les handlers (`space_access`), `instance_administrator` dans `whoami`, actions `native_remote` `listSpaces`/`spaceCapabilities`, types TS `packages/ui/src/api/catalog-types.ts` ; contrat `docs/contracts/ui-catalog-and-capabilities.md` ; aucune migration | Vérifiée sur PostgreSQL jetable : `native_catalog` (accès direct, par équipe, sans accès, révocation entre liste et ouverture, retrait d'équipe, homonymes, pagination et balayage borné, curseur d'un autre principal/altéré refusé, bornes de `limit`, capacités propriétaire/membre/équipe/extérieur/limité, `whoami`) + tests unitaires curseur et client ; fmt, clippy workspace et Tauri | Sélecteur de personnes livré avec les annuaires autorisés en L5 (limites §2). La preuve L0 d’accès limité utilise un authorizer de test ; la recette native distincte utilise une vraie politique de contenu ; `lastActivityAt` ne couvre que les événements de dépôt |
| L1 Fondations | Fait (`packages/ui` : tokens sombre/clair, polices locales OFL, thème précoce, routeur hash, shell, composants, diff) | Tests renderer + captures sombre/clair de Changements comparées à la maquette | Contraste clair mesuré à refaire par outil ; palette clavier couverte en L6 (ouverture, Échap, focus, recherche Git) |
| L2 Entrée/navigation | Fait : shell routé Web + Desktop ; connexion, compte, apparence ; projets Web par nom depuis le catalogue L0 (y compris sur lien direct) | Renderer + recettes natives Web/Desktop réécrites par L3/L4/L5 | Nom de projet sur lien direct résolu en parcourant au plus 5 pages du catalogue du compte |
| L3 Desktop | Fait : Synchronisation P11 en panneau permanent du bandeau (serveur/compte, Publier et Recevoir distincts, transfert en cours avec étapes observées et annulation, « publication à confirmer » distincte d’un échec, activité conservée après redémarrage, transfert interrompu signalé) + route `/sync` (reprise, avancé : publier en gardant les têtes, créer sur le serveur, captures candidates et choix des têtes, passage en accès limité) ; connexion P02 dédiée `/connect` (code lisible, vérification automatique bornée à l’intervalle serveur, expiration/refus, certificat d’entreprise en étape avancée ; serveur mémorisé par dossier, autorité publique par serveur, jetons toujours dans le coffre OS) ; projet d’équipe choisi par nom dans le catalogue L0 (assistant Récupérer et Projets « Dans l’équipe »), référence en option avancée ; Fusion P10 `/merge[/:conflict]` (liste, comparaison base/variantes en `DiffView`, choix variante/passage/texte, aperçu final lié à `expectedOperation`/`expectedStep`, brouillons conservés par Layer/passage) ; Historique P07/P08 (captures filtrées et groupées, opérations locales, restauration par aperçu), Layers P09, Réglages P17 (capture locale, identité, serveur, réécriture), Diagnostic P23 sans panneaux hérités ; accès limité dans le même shell (Changements, Fichiers, Historique, Équipe, Fusion, Synchronisation, Réglages, Diagnostic) avec actions selon capacités, pages projet complet refusées, appels au magasin local sérialisés ; Équipe encadre les surfaces partagées (revues, versions, historique publié, fichiers distants, administration ; revues à portée limitée) | `pnpm check` vert ; `pnpm test:e2e:renderer` 18/18 (6 nouveaux tests Desktop : connexion + publication/annulation même `transferId`, publication non confirmée, fusion avec préconditions, aperçu périmé refusé, accès limité, clone par nom) ; recettes natives réelles sur PostgreSQL jetable (Docker) : `desktop` 5/5 (4 SCM + équipe), `desktop-partial` 1/1 ; captures sombre/clair 1440 et 960 px sur données fictives | Panneaux retirés : `NativeRemotePanel`, `NativeMergePanel`, `NativeConflictBlocks`, `NativeConflictComparison`, `NativePartialMerge`, `NativeHistoryPanel`, `NativeIdentityPanel`, `NativeDiagnosticsPanel`, `NativeChoosePanel`, `NativePartialWorkbench`, `NativeRewriteImpact`, `NativeReviewPanel`. L6 remplace les anciennes surfaces complètes d’Équipe par `collab` et `team` ; `NativeProjectFiles` (Fichiers accès limité) et `NativeScopedReviews` conservent leurs contrats spécifiques. Limites : pas d’octets/pourcentage de transfert (le moteur n’en publie pas) ; le navigateur n’est pas ouvert automatiquement (lien + copie) ; accès limité : Layer cible par identifiant (pas de liste de Layers pour une projection), capture publiée à fournir pour un clone limité, reprise manuelle au choix (le journal n’indique pas l’opération) ; pas de page capture P08 dédiée (pas de diff capture/parent local) ; capture automatique en pause par session seulement (pas de commande de réglage par Layer) |
| L4 Collaboration | Web fait : composants partagés sans transport imposé dans `packages/ui/src/collab` (Fichiers P06, Historique P07 et capture en lecture seule P08, comparaison, Revues P12 avec proposition, détail P13 onglets Discussion/Fichiers/Validations, validation et journal P13b, Versions et repères P14 avec publication/révocation) ; pages Web une par fichier (`apps/studio-web/src/pages/{Files,History,Reviews,ReviewDetail,Run,Versions}Page.tsx`), routes `/history/:capture`, `/reviews/:review?version=&tab=`, `/reviews/:review/runs/:run` ; indices de capacités L0 appliqués (action désactivée avec raison) | Recette native Web réelle (`node scripts/test-native-ui-ci.mjs web`, PostgreSQL jetable) : connexion, projet ouvert par son nom de catalogue, fichiers et droits, historique d’un chemin et comparaison, commentaire, retour perdu puis relance sans doublon (vérifié au serveur), versions 2 et 3 d’une proposition avec décision précédente affichée comme caduque et version 2 en lecture seule, brouillon restauré après rechargement, preuve et journal CI, refus de la Gate de publication sans événement ajouté, comptes (surface héritée), stockage sans jeton/mot de passe/donnée projet, déconnexion puis rechargement ; tests de modèle purs dans `pnpm test:e2e:renderer` ; captures Web sombre/clair 1440 et 800 px | Desktop adopte les composants `collab` via `pages/TeamPage.tsx` et `remoteTeam.ts` : Revues et Contenu publié (fichiers/historique/versions), routes par revue/capture/version et brouillons isolés. La qualification L6 est distincte des preuves Web ci-contre. Accès limité vérifié localement avec `LAYRS_NATIVE_WEB_RESTRICTED=1` (politique de chemins appliquée par un Desktop Tauri) : contenu verrouillé sans identifiant, historique expliqué (403 à l’API), capacité d’octets refusée, projet fermé après refus du bootstrap ; non exécuté par défaut en CI Linux |
| L5 Administration | Fait côté Web : Équipe et accès (Membres/Invitations/Équipes), Accès aux fichiers, Réglages (Général, Protections, Automatisations, Webhooks, Avancé ; garde de sortie), Boîte de réception (Invitations/Activité), Administration (Comptes, Comptes de service, Stockage, État du serveur), sessions du Compte ; composants `packages/ui/src/team/` pilotés par transports ; transport de politique Web (`apps/studio-web/src/app/policy/`, BLAKE3 via `@noble/hashes`, JCS vérifié contre `layrs-sync`) ; capacités L0 et `instanceAdministrator` utilisés comme indices | Recette native Web réelle (`fixtures/nativeWebTeam.ts`) : comptes, compte de service et secret unique, stockage, état, rôles lus, équipe + historique, invitation par nom puis acceptation par le compte invité, droits propres, protection + garde, webhook + secret, exception de fichier appliquée puis vérifiée par le serveur, retour à la gestion par équipes ; `pnpm check` ; renderer ; captures sombre/clair 1440/800 | Desktop adopte les composants `team` avec transport Tauri : Équipe, Accès aux fichiers, Réglages serveur et administration d’instance gardée par whoami. La recette `desktop-native-team` est adaptée ; voir L6 pour sa preuve actuelle. |
| L6 Git/finitions | Fait le 5 octobre : assistant Git Tauri, composants collaboration/administration Desktop, session liée au compte, reprise exacte des politiques et documentation courante | `pnpm check` réussi ; renderer 27/27 ; recettes réelles SCM, équipe, Git, accès limité et Web ; binaire Windows embarqué reconstruit ; constats des revues indépendantes corrigés | [Qualification L6 : commandes, preuves, empreinte et limites](04-qualification-l6.md). Tests locaux exécutés en plusieurs commandes ; aucune distribution signée ni CI distante annoncée |
| Création Web et lenses par projet, après L6 | Projets › Nouveau projet, organisations créables listées par droits, pins exacts à la création ; Administration › Lenses pour importer/retirer/révoquer ; Réglages › Lenses par projet avec révision attendue ; Web/Desktop téléchargent les viewers vérifiés, affichage isolé et secours ; aperçu texte publié contrôlé par chemin | `native_catalog` PostgreSQL : création partagée, refus membre, suspension, réessai exact, pin/association initiale, retrait/révocation/historique ; Web HTTPS : 2 tests dont import Markdown, création projet et rendu du README publié ; 24 tests renderer sélectionnés dont 6 lenses ; cinq tests du vérificateur de paquet avec WASM réel ; `pnpm check` complet (migrations 20, format, Clippy workspace, TypeScript, builds Web/Desktop), contrats SDK TS et paquets intégrés validés | [Contrat de distribution](../../contracts/lens-distribution-v1.md) et [guide](../../guides/projects-and-lenses.md). Qualification Desktop native de ce parcours réussie ; distribution externe encore en cours ; historique L6 ci-dessus inchangé. Analyseur de paquet indisponible, image binaire non rendue, worker JS sans quota mémoire dur. |

## 6. Qualification significative

Complément du 5 octobre : contextes entreprise/organisation/personnel et équipes
d'organisation intégrés. Les deux options serveur sont indépendantes et inactives
par défaut. `pnpm check` passe avec 23 migrations ; PostgreSQL vérifie les quatre
configurations, la réinitialisation, la migration conservatrice et les droits
personnels. Les équipes réutilisables sont vérifiées sur plusieurs projets,
y compris rollback et révocation à la limite des versions. Le fichier renderer
Desktop passe 19 tests ; la recette Desktop native HTTPS et les trois tests Web
HTTPS passent. Administration → Organisations permet la bascule à chaud avec
choix principal, avertissement et confirmation saisie (identifiant unique si
homonymes). Les organisations masquées gardent projets, membres et droits ;
les projets personnels restent indépendants. La navigation Web/Desktop présente
le sélecteur d'organisation puis « Tous les projets » et les projets ; créer
une organisation est le dernier choix du menu. Le menu disparaît en mode mono.
`organisation_mode`, `native_contexts` et `native_sync_v3` passent, notamment
le refus d'anciens téléchargements/reçus pendant le masquage puis leur retour.
Les recettes Web et Desktop valident la bascule et la restauration ; Web couvre
aussi les homonymes et l'annulation. Les revues UI et sécurité sont favorables.
La concurrence forcée et la pause/reprise des webhooks restent sans recette
dédiée ; les anciens processus serveur doivent être remplacés avant la bascule.
Le modèle,
les API et la limite d'adhésion via un premier projet sont documentés dans le
[contrat des contextes](../../contracts/project-contexts.md).

Correction du parcours d'autorisation d'appareil : `/auth/device.js` est routé
vers le serveur au lieu du HTML Studio. La page réutilise une session Web ouverte
sur la même origine, sans transférer le jeton, avec choix entre comptes et
confirmation explicite liée à une vérification de courte durée. La connexion
manuelle reste disponible. Quatre régressions HTTPS réelles passent en plus des
trois recettes Web précédentes (**7/7**) ; `pnpm check` passe. La revue critique
a vérifié l'invalidation des sessions et des demandes, les réponses HTTP 204 et
le nettoyage des sessions temporaires. L'instance locale reconstruite sert les
ressources corrigées et conserve ses projets et adhésions.

Finition du 6 octobre : la page d'autorisation reprend logo, tokens et polices
de Studio, thème partagé et adaptation mobile. Ses assets sont embarqués et
vérifiés par la recette HTTPS ; vues sombre/claire/mobile/confirmation inspectées.
Le catalogue Desktop est corrigé pour la connexion depuis Nouveau projet avant
choix du dossier : cette connexion notifie le catalogue global, et un compte
sans organisation voit un état vide. Six tests renderer ciblés passent, ainsi
que la recette Tauri réelle désormais démarrée depuis ce parcours et les sept
tests Web. `pnpm check` passe ; serveur local et exécutable embarqué actualisés.

### Contrats et confidentialité

Catalogue filtré par droits individuels/équipe ; révocation entre liste et ouverture ;
curseur valide seulement dans son contexte ; aucune recherche de personnes hors
du périmètre autorisé ; erreur sans fuite de contenu. Autorisations serveur
vérifiées après la présentation de capacités. Mêmes garanties pour aperçus,
commentaires, historique, notifications, chemins et fichiers réservés.

### Composants et interactions

Choix du thème avant rendu, persistance et changement système ; mouvements réduits ;
navigation clavier et focus ; rendu long/étroit ; erreur conservant la saisie ;
réponse tardive ignorée après changement de compte/projet ; double soumission.
Tests des comportements, pas des classes CSS. Inspection visuelle distincte.

### Parcours réels

Réutiliser `studio-web-native.spec.ts`, `desktop-native-scm.spec.ts`,
`desktop-native-team.spec.ts`, `desktop-native-git.spec.ts`, `desktop-native-partial.spec.ts` et leurs fixtures
HTTPS/Tauri. Adapter les sélecteurs accessibles au nouveau vocabulaire ; ne pas
supprimer un scénario parce que son ancien bouton a disparu.

Inclure connexion/session expirée/mot de passe modifié, catalogue vide, création,
capture automatique, comparaison, restauration, transferts, revue/version exacte,
permissions par fichier, conflit partiel et interruption/reprise. Vérifier les
transports réels sur données jetables, jamais modifier les projets utilisateur
pour rendre une démonstration jolie.

### Rendu et ressenti

Comparer la nouvelle page Changements à la maquette approuvée au même viewport ;
puis chaque autre famille de page aux tokens et composants, pas à une image unique.
Capturer Web/Desktop en sombre et clair, 1440 px et petite fenêtre, avec vide,
chargement, erreur, accès limité et longue liste. Tester clavier seul et zoom.
Consigner les différences restantes ; une capture seule ne prouve pas les actions.

Le navigateur/outil de capture sera choisi dans le workflow de validation en
respectant les outils autorisés ; ne pas présenter une image générée comme résultat
de recette. Windows et Docker Linux disponibles ; macOS/ARM non qualifiables ici.

### Porte de sortie

- Chaque P01–P23 et sous-vue possède son état de couverture dans le journal.
- Parcours critiques du document 01 exécutés, erreurs connues corrigées ou limites
  métier nommées ; aucune fonction existante disparue sans remplacement documenté.
- Catalogue réel, droits et états métier vrais ; pas de données de démonstration.
- Thèmes et animations accessibles, navigation stable, pas de fuite de secrets.
- Contrôles TypeScript/builds, tests ciblés puis recette intégrée réussis ; refaire
  une campagne plus large seulement si changements ou risques le justifient.
- Web local reconstruit et version Desktop de recette identifiée ; journaux,
  commits et guide de test présents. Ne pas déclarer un MSI signé ou distribué
  parce qu'un build local s'est ouvert.

### Limites métier constatées pendant l’implémentation

- **Message de capture** (maquette P05) : le moteur n’en porte pas. Une Step est
  adressée par son contenu et le résumé d’Operation reste local au poste. La page
  Changements l’annonce et renvoie vers les repères/versions nommés. Ajouter un
  message partagé demande un contrat (observation ou métadonnée publiée) à décider.
- **Intégrer une revue depuis le Web** (P13) : aucune route ; l’intégration est une
  transaction publiée depuis Desktop sur le Layer cible. Le bouton « Intégrer » reste
  visible, distinct d’« Approuver », désactivé avec cette raison.
- **Décisions sur une version ancienne** : le serveur refuse approbation et demande de
  modifications quand la version date d’avant un changement de droits ou de protection
  (`review_decision_not_current`) ; la revue l’annonce et propose une nouvelle version.
  Les décisions de l’auteur ne comptent pas sans auto-approbation, ce que le client ne
  peut pas savoir (protection lisible par les administrateurs seulement).
- **Noms des personnes** (P07/P12/P13) : pas d’annuaire lisible par un membre. Seul
  « Vous » est nommé ; les autres restent « Autre compte », identifiant en détails.
  Les participants d’une proposition se saisissent encore par référence (champ avancé).
- **Aperçus de fichiers** (P06) : le texte UTF-8 publié jusqu'à 1 Mio est lu
  par un transport lié au chemin exact et aux droits, puis affiché par une lens
  ou en repli. Un contenu interdit ne fournit ni octets ni aperçu. Image binaire
  et fichiers plus grands restent sans aperçu de contenu.
- **Validations** (P13) : l’API liste toutes les exécutions du projet par pages ; le
  filtre « Capture proposée » porte sur les pages chargées et le dit.
- **Accès limité** : l’historique complet est refusé par le serveur (403) ; la page
  l’explique sans requête. Le scénario natif correspondant exige les clients partagés
  (Desktop Tauri pour appliquer une politique de chemins) : clients partagés ou
  option `LAYRS_NATIVE_WEB_RESTRICTED=1`.
- **Capture P08 Web** : pas de route de détail ; lecture de l’historique ancré sur la
  capture (une entrée). Restauration réservée à Desktop.
- Sur un lien profond Web, le nom est désormais résolu depuis le catalogue du
  compte, avec un parcours borné à cinq pages ; la référence sert de repli.

### Limites constatées pendant L5

- **Règle de fichier = règles personnalisées.** Toute politique ajoutée hors
  équipes/rôles (exception de fichier incluse) fait passer le projet en mode
  personnalisé : rôles, équipes et invitations sont suspendus jusqu’au retour à
  la politique gérée, qui retire ces règles. L’aperçu l’annonce ; combiner
  équipes et exceptions de fichiers demande un contrat serveur.
- **Transport de politique Web côté client.** Le serveur n’offre pas d’aperçu de
  politique : le Web construit la transaction (identités BLAKE3/JCS vérifiées
  contre `layrs-sync`) ; le serveur recalcule tout et reste seul juge. Sans
  journal durable côté Web, une réponse perdue se reprend en réessayant le même
  aperçu tant que la page reste ouverte.
- **Droits d’autrui calculés.** Les droits d’une autre personne sont expliqués à
  partir des règles lues ; seuls vos propres droits sont confirmés par la
  projection du serveur. Les règles ne sont lisibles que par les administrateurs.
- **Recette CI par référence.** Le serveur ne renvoie pas le contenu de la
  recette : modifier une protection avec validation automatique exige de
  recoller la recette exacte (annoncé dans le formulaire).
- **Sélection de personnes.** Pas d’annuaire borné pour un administrateur de
  projet : membres de l’organisation par nom, sinon identifiant technique en
  repli. Une invitation reçue pour un projet encore inaccessible n’affiche pas
  son nom. Un webhook désactivé ne se réactive pas.
- **Desktop.** Les composants partagés sont désormais raccordés au transport Tauri.
  Les mutations et aperçus de politique prennent le verrou de travail du dossier.
  Les revues des projections limitées gardent leur parcours natif distinct.

## 7. Suggestions différées, sans confusion de périmètre

À envisager après ce chantier : regroupement global de notifications, assignation
de revues, mentions, édition de profil/avatar, galerie d'assets plus riche, aperçu
des formats supplémentaires. Ces idées demandent des contrats produit distincts.
Elles ne sont pas nécessaires pour rendre la navigation actuelle moderne et fiable.
