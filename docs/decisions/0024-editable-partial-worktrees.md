# 0024 — Worktrees partiels éditables et chemins réservés

Date : 21 septembre 2026. Statut : décision produit acquise, implémentation en cours.

## Comportement retenu

Chaque membre peut avoir des droits de lecture et de modification distincts sur
un fichier ou un sous-arbre. Le nom et le chemin restent visibles ; le contenu
interdit n'est pas téléchargé. L'interface affiche un cadenas et réserve cet
emplacement. Créer, renommer ou déplacer un fichier sur un emplacement réservé
est refusé, y compris en remplaçant un répertoire parent. Les alias de casse et
de chemins propres au système de fichiers ne doivent pas contourner ce refus.

Un outil externe peut créer un fichier sur le disque. Layrs conserve ces octets,
signale la collision et refuse leur capture/publication à cet emplacement. Un
fichier interdit absent du disque n'est jamais interprété comme une suppression.
Une révocation ne prétend pas effacer les copies précédemment obtenues.

## Invariants

- Les droits affichés proviennent d'une autorité serveur observée, liée au
  membre, Space, Step, PolicySet et epoch. Un état incomplet ou périmé ne permet
  pas de mutation. Le serveur revalide les droits à la publication.
- Écrire nécessite lire et écrire ; un refus reste prioritaire. Aucun simple
  identifiant d'objet ne permet de copier des octets interdits vers un chemin
  accessible. Les politiques v1/v2 conservent leurs garanties existantes.
- Une projection locale a un registre et un format distincts d'un Store natif
  complet. Une structure distante connue n'est pas une fermeture locale admise.
  Les anciens lecteurs doivent refuser ce nouveau format avant toute mutation.
- Le client soumet des modifications de chemins avec leurs attentes exactes.
  Le serveur reconstruit les Trees canoniques en préservant toutes les entrées
  hors modification, puis applique les Gates, CAS, reçus et règles de rejeu.
- Pas de faux fichier vide, de Tree filtré conservant son ancien identifiant,
  ni de suppression des gardes du clone complet pour simuler ce comportement.

## Découpage et preuves attendues

1. Moteur : réservations, format local, capture sélective, matérialisation,
   collisions, reprise et migration sans perte d'historique.
2. Serveur : politiques lecture/écriture, exploration paginée ancrée, patchs
   autorisés atomiques, uploads liés au chemin, rejeu et changement d'autorité.
3. Interface : chemins verrouillés, validation des destinations, sélection
   fichier/membre et aperçu d'un remplacement de politique réellement éditable.
4. Intégration : deux membres HTTPS, fichier autorisé modifié et fichier interdit
   intact ; collisions externes, révocation, réautorisation, panne et concurrence.

Les trois fondations peuvent avancer en parallèle ; leur présence ne clôt pas
la fonctionnalité avant raccordement et qualification des parcours complets.
Les journaux `PARTIAL-STORE`, `PARTIAL-SERVER` et `PARTIAL-UI` sous
`docs/plans/pme/journal` indiquent les preuves et le travail restant.

Cette décision complète ADR0023 : ses protections v2 restent valables pour les
anciens parcours. Les artefacts qualified4 précèdent ce travail et ne le livrent
pas. Aucune migration, installation ou qualification nouvelle n'est revendiquée
par ce document.
