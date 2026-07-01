# ADR 0002: Pas de Git dans le coeur V1

Date: 2026-06-29

Statut: accepté

## Contexte

Layrs se positionne comme une alternative local-first au source control et aux plateformes type GitHub. Reprendre Git comme coeur imposerait son modèle de commits, branches, index et refs, alors que Layrs doit modéliser directement Layers, Weaves, Proofs, Gates, Policies et Graph.

## Décision

Le coeur V1 de Layrs ne garantit aucune compatibilité Git et ne stocke pas ses états comme un dépôt Git interne.

Git peut devenir un connecteur d'import, d'export, de migration ou d'interopérabilité plus tard. Ce connecteur ne doit pas dicter le modèle de données central.

## Conséquences

- Les noms et objets produit ne sont pas des synonymes de concepts Git.
- Les APIs internes doivent exposer les concepts Layrs, pas des abstractions Git déguisées.
- Les futures migrations Git devront traduire explicitement les concepts au lieu de supposer une équivalence parfaite.
- Les tests V1 doivent valider le modèle Layrs indépendamment de Git.
