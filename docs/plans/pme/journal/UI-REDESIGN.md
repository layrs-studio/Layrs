# Refonte UI/UX Web et Desktop — 23 septembre 2026

## Demande et état

Refondre entièrement les interfaces Layrs pour les petites entreprises :
thème sombre par défaut configurable, interactions agréables, animations et
retours d'état fréquents. Autorisation de remplacer la présentation existante.
Les capacités et protections métier restent à conserver.

État : direction visuelle validée ; plan détaillé préparé le 24 septembre 2026.
Voir [le plan de référence](../../../design/redesign/README.md), ses pages,
principes d'interaction et lots d'exécution. Implémentation produit non commencée.
Trois maquettes indépendantes présentées dans la conversation, dans cet ordre :

1. `Atelier` : travail local et comparaison textuelle, charbon et menthe.
2. `Orbital` : accueil projets et revues, encre et pervenche.
3. `Studio cuivre` : collaboration autour des assets, charbon chaud et cuivre.

Ce sont des directions artistiques illustrées sur différents parcours, avec
des données fictives. Aucune n'est encore une interface fonctionnelle. Le choix
visuel sera appliqué à toutes les surfaces, pas uniquement à l'écran illustré.
Les images sont visibles dans la conversation ; aucune image générée n'est
utilisée comme fond ou remplacement d'une interface interactive.

L'utilisateur a validé la première direction révisée pour son logo. Il demande
maintenant un plan détaillé décrivant chaque page et la logique des regroupements,
avec propositions pour une expérience moderne. Aucun fichier produit ni service
modifié pendant cette phase de planification.

### Révision fondée sur la marque

- Source utilisateur : `C:/Users/ALEXA/Downloads/Group 1.svg`, inspectée et copiée
  sans modification dans `docs/design/layrs-brand/icon.svg`.
- Wordmark : `layrs` en Audiowide, selon la référence fournie. Réserver cette
  police à la marque ; interface en sans-serif lisible, code en monospace.
- Couleurs originales du SVG : corail `#F43F5E`, ambre `#F59E0B`. Remplacer
  les accents menthe par ces couleurs sur une base graphite. Le vert reste
  réservé aux succès et ajouts dans les comparaisons.
- Conserver la structure de la première proposition. Titre sobre « Changements »
  sans serif, une seule action principale « Créer une capture », progression et
  états lisibles, choix sombre/clair/système. Animations à implémenter ensuite.
- [Maquette révisée](../../../design/layrs-brand/atelier-revised.png), produite
  par ImageGen avec la première proposition et le logo utilisateur comme références.
  Brief : même organisation, vraie identité Audiowide/corail/ambre, écran de
  changements graphite, comparaison et retours discrets. Le logo de cette image
  reste un rendu généré ; l'interface finale devra utiliser le SVG original.

## Diagnostic établi

Source : capture utilisateur et lecture du code, avec investigation indépendante.
Pas de parcours navigateur ou Desktop exécuté pendant cette phase.

- Le Web monte `NativeStudio`, pas l'ancien `StudioApp`. Il empile ouverture
  manuelle par identifiant, administration, fichiers, historique et revues.
- Desktop monte par défaut `NativeWorkbench` ; les projets partiels ont
  `NativePartialWorkbench`. Les fonctions locales et serveur sont empilées.
- `#local` expose encore l'ancien `DesktopApp`. Ses liens `#desktop-*` font
  remonter le nouveau workbench : défaut de routage identifié dans le code,
  à reproduire et corriger lors de l'unification.
- Les surfaces communes sont dans `packages/ui/src/native-*` : administration,
  revues, versions, historique, fichiers et permissions.
- Les tokens sont sombres fixes, sans choix de thème ni traitement global
  identifié de la préférence de mouvements réduits.
- Le premier écran ne guide pas vers une action utile. Les réglages techniques
  occupent l'espace, les identifiants remplacent les noms, les formulaires sont
  trop larges et la hiérarchie typographique est insuffisante.

## Plan d'implémentation après choix

### A. Fondations communes — à faire

Définir tokens de couleurs, typographie, espacements, surfaces, bordures et états.
Créer un shell Web/Desktop cohérent, navigation accessible et composants communs
boutons, champs, dialogues, menus, onglets, états vides et notifications.
Thème sombre initial ; choix clair/sombre/système mémorisé localement et appliqué
avant affichage. Navigation clavier, focus visible, libellés et contrastes.
Transitions de 120–200 ms, ouverture de panneaux de 180–240 ms ; désactivation
des déplacements et effets non essentiels avec `prefers-reduced-motion`.

### B. Accueil, connexion et projets — à faire, dépend de A

Repenser la connexion, la première utilisation et l'ouverture de projet par nom.
Vérifier les contrats de découverte/création accessibles au compte courant ;
ne pas inventer de catalogue client ni exposer les projets non autorisés.
Prévoir chargement, aucun projet, accès refusé, session expirée et reconnexion.
Sur Desktop : ouvrir/créer un dossier et reprendre un projet récent.

### C. Travail local Desktop — à faire, dépend de A

Séparer changements, captures, Layers et synchronisation. Liste de fichiers et
comparaison lisible ; états texte/binaire/interdit distincts. Capture automatique
visible mais discrète. Préserver les verrous d'opération, aperçus avant application,
projets partiels, collisions externes et protections de chemins réservés.
Refonte des conflits et rebase sans modifier les décisions métier.

### D. Projet Web et collaboration commune — à faire, dépend de A

Vues fichiers, historique, revues et versions séparées. Détail de revue avec
contexte, comparaison, commentaires et décisions compréhensibles. Actions
primaires explicites, formulaires secondaires en panneaux. Conserver pagination,
brouillons, invalidation de session/scope et contrats de transport.
Les aperçus d'assets illustrés dans les maquettes doivent correspondre aux
formats réellement supportés ; pas de fonctionnalité simulée dans le produit.

### E. Équipe et réglages — à faire, dépend de A

Séparer invitations, membres, permissions par fichier, protections, intégrations,
appareils et administration d'instance. Exposer les options selon les droits
effectifs. Distinguer visibilité du nom et accès au contenu. Actions sensibles
avec explication et confirmation contextualisées. Diagnostic technique secondaire.

### F. Retours utilisateurs — à faire avec B/C/D/E

Chargements locaux, progression réelle pour les opérations longues, succès après
confirmation du serveur, erreurs près de l'action avec reprise utile. Éviter les
succès anticipés et les boutons silencieux. Annonces accessibles via zones live.
Préserver les champs saisis après erreur ; éviter les notifications répétitives
pour les captures automatiques. Annulation seulement si le moteur la supporte.

### G. Intégration et qualification — à faire après B/C/D/E

Vérifier les parcours natifs existants Web/SCM/équipe/partiel et l'ancien routage
encore accessible. Captures comparables aux maquettes, responsive à 1280/1440 et
petite fenêtre, thèmes, clavier, réduction des mouvements, états vide/erreur/attente.
Ne pas valider uniquement un écran rempli de données fictives. Vérifier que les
actions réelles restent disponibles. Mise à jour du build local pour consultation.
Windows et Docker Linux seulement disponibles ; ne pas revendiquer macOS/ARM.

## Parallélisation et commits

A définit d'abord les contrats et tokens. Ensuite B-Web/D, C-Desktop et E peuvent
avancer en parallèle avec propriétaires de fichiers distincts. D et E se
coordonnent dans `packages/ui` ; aucun remplacement concurrent du même shell.
F accompagne chaque lot. G s'effectue sur le résultat intégré avec revue distincte.
Commit par lot cohérent validé, mise à jour de ce journal à chaque livraison.

## Reprise immédiate

La direction est validée ; ne pas redemander un choix visuel. Le plan détaillé
`docs/design/redesign` remplace les grandes étapes A–G ci-dessus pour l'exécution.
La demande actuelle est le plan ; aucune implémentation anticipée. Quand le travail
d'implémentation reprend, commencer les contrats L0 et fondations L1 avec leurs
responsabilités séparées. Utiliser les vrais flux existants et le SVG utilisateur,
sans remplacer le produit par une démonstration. Mettre à jour le tableau de suivi
dans `03-delivery.md`, les validations et les commits à chaque étape.

### Compléments vérifiés pour le plan détaillé

Investigation indépendante des contrats : catalogue de projets accessibles absent
du V3, capacités effectives non synthétisées, changement de mot de passe existant,
notifications par projet, invitations globales, pont Git CLI sans action Tauri,
aperçus distants à raccorder au transport protégé. Ces écarts sont explicitement
planifiés ; aucun n'est présenté comme une simple retouche graphique.
