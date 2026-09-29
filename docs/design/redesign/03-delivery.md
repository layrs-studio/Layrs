# Exécution, dépendances et qualification

Plan préparé le 24 septembre 2026. Ce document sert de registre de reprise.
Les cases cochées indiquent des livrables réels, pas une intention.

## 1. État initial vérifié

- Web actif : `apps/studio-web/src/NativeStudio.tsx` monté par `main.tsx`.
- Desktop actif : `NativeWorkbench.tsx`, branche `NativePartialWorkbench.tsx`.
  L'ancien `DesktopApp` (`#local`) et l'ancien `StudioApp` Web ont été retirés
  lors du nettoyage du 29 septembre 2026, avec le moteur local legacy.
- Surfaces communes : `packages/ui/src/native-*` ; styles partagés et styles
  Desktop séparés. L'ancien `NotificationProvider` (inutilisé) a été retiré ; le
  récupérer depuis l'historique git (`packages/ui/src/studio-components.tsx`) ou
  le réécrire pour la refonte.
- Contrats de commandes/transports, garde de génération, brouillons et exclusions
  concurrentes sont à préserver pendant extraction en vues.
- Image Lens existe pour PNG/JPEG/WebP avec données d'aperçu fournies ; le navigateur
  de fichiers distant natif ne raccorde pas encore ces données.

Cette analyse repose sur lecture de code et capture utilisateur. Elle ne remplace
pas une recette de toutes les pages actuellement exécutées.

## 2. Registre des contrats indispensables

| Besoin UX | État réel | Travail prévu / garde-fou |
| --- | --- | --- |
| Projets par nom | Aucun catalogue V3 accessible au compte identifié dans les routeurs ; ancien registre Desktop uniquement local | Ajouter catalogue paginé autorisé, groupement par organisation, noms et contexte ; tester accès direct et par équipe, révocation, curseur et homonymes |
| Créer un projet serveur | `POST /v3/workspaces/provision` existe ; workspace existant réservé à son propriétaire | Raccorder formulaire Web, noms/slug validés et aide ; refléter la restriction, ne pas offrir arbitrairement toutes les organisations |
| Connaître la personne | `GET /v3/auth/whoami` fournit principal, acteur, nom et kind | Charger le profil d'affichage ; ne pas déduire tous les droits d'administration du seul kind |
| Actions autorisées dans une page | Bootstrap/policy-management ne sont pas une synthèse complète des capacités effectives | Définir une réponse de capacités UI dérivée des mêmes contrôles serveur ; aucune autorisation client autonome ; contrôle serveur conservé à chaque mutation |
| Choisir un compte à inviter | Liste globale de comptes réservée admin, liste de membres éligibles aux équipes disponible | Définir le sélecteur adapté au rôle ; si recherche de comptes nécessaire, endpoint borné avec politique d'exposition explicite ; ne pas ouvrir la liste globale à tous |
| Changer le mot de passe | `POST /v3/auth/password` existe et invalide les jetons | Ajouter le formulaire et la reconnexion ; pas d'édition de nom/avatar inexistante |
| Invitations et notifications | Inbox invitations globale ; notifications et lecture par projet | Commencer par onglets Invitations / Activité du projet ; différer l'agrégation globale sans badges trompeurs |
| Aperçus distants | Liste native : chemin, objet et droits ; plans/transferts V3 protégés existants | Assembler les contenus autorisés via ces contrats, borner tailles et isoler formats ; pas d'URL publique de miniature inventée |
| Import/export Git Desktop | Pont CLI existant, pas d'action Tauri dédiée identifiée | Adapter une commande structurée vers le cœur/bridge, analyse avant écriture et recette indépendante ; pas de shell composé depuis un champ |
| Thèmes/Audiowide | Tokens sombres fixes, police non intégrée | Tokens sémantiques, police locale/licence, préférences locales initialisées avant rendu |

**Obligatoire pour la refonte utile :** catalogue, informations de capacité et
sélection de personnes sans IDs manuels. Ces compléments backend sont de vrais
lots testés, pas des détails CSS. Le design final des DTO/routes est arrêté au
lot L0 après examen du stockage et de l'évaluation des permissions. Tout éventuel
changement de schéma reçoit sa propre migration et ses tests ; aucune migration
automatiquement supposée nécessaire dans ce plan.

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
| L3 | Desktop local | `NativeWorkbench`, `NativePartialWorkbench`, panneaux locaux et styles propres |
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
| L0 Contrats | Non commencé | Non exécutée | Catalogue et capacités V3 à spécifier |
| L1 Fondations | Non commencé | Non exécutée | Police locale, tokens, shell, composants |
| L2 Entrée/navigation | Non commencé | Non exécutée | Dépend de L0/L1 |
| L3 Desktop | Non commencé | Non exécutée | Préserver complet/partiel |
| L4 Collaboration | Non commencé | Non exécutée | Composants partagés |
| L5 Administration | Non commencé | Non exécutée | Revue sécurité requise |
| L6 Git/finitions | Non commencé | Non exécutée | Bridge Tauri et recette complète |

## 6. Qualification significative

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
`desktop-native-team.spec.ts`, `desktop-native-partial.spec.ts` et leurs fixtures
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

## 7. Suggestions différées, sans confusion de périmètre

À envisager après ce chantier : regroupement global de notifications, assignation
de revues, mentions, édition de profil/avatar, galerie d'assets plus riche, aperçu
des formats supplémentaires. Ces idées demandent des contrats produit distincts.
Elles ne sont pas nécessaires pour rendre la navigation actuelle moderne et fiable.
