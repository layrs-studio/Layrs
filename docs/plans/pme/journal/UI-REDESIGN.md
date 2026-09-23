# Refonte UI/UX Web et Desktop — 23 septembre 2026

## Demande et état

Refondre entièrement les interfaces Layrs pour les petites entreprises :
thème sombre par défaut configurable, interactions agréables, animations et
retours d'état fréquents. Autorisation de remplacer la présentation existante.
Les capacités et protections métier restent à conserver.

État : inventaire du code et première exploration visuelle terminés.
Trois maquettes indépendantes présentées dans la conversation, dans cet ordre :

1. `Atelier` : travail local et comparaison textuelle, charbon et menthe.
2. `Orbital` : accueil projets et revues, encre et pervenche.
3. `Studio cuivre` : collaboration autour des assets, charbon chaud et cuivre.

Ce sont des directions artistiques illustrées sur différents parcours, avec
des données fictives. Aucune n'est encore une interface fonctionnelle. Le choix
visuel sera appliqué à toutes les surfaces, pas uniquement à l'écran illustré.
Les images sont visibles dans la conversation ; aucune image générée n'est
utilisée comme fond ou remplacement d'une interface interactive.

En attente du choix utilisateur avant implémentation, suivant la phase ideate
du skill Product Design. Aucun fichier produit ni service modifié dans ce lot.

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

Demander la direction 1/2/3 ou les ajustements souhaités. Le nombre correspond
à l'ordre des trois images affichées dans la conversation, pas à une génération
ultérieure. Après choix, charger le workflow image-to-code, adapter au dépôt
existant sans créer un projet de démonstration à la place du produit, commencer A.
