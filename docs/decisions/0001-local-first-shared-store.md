# ADR 0001: Local-first shared store

Date: 2026-06-29

Statut: accepté

## Contexte

Layrs doit rester utile même sans serveur central. Les Spaces, Layers, Artifacts, Weaves, Proofs et Graphs doivent pouvoir être créés, inspectés et reliés localement avant toute synchronisation.

## Décision

La V1 adopte un store partagé local-first comme source de vérité du coeur Layrs. Les surfaces produit liront et écriront dans ce store via des API internes, au lieu de dépendre d'un service distant comme autorité primaire.

Le store partagé doit représenter les objets Layrs et leurs relations. La synchronisation, la collaboration distante et les ponts externes seront construits autour de ce modèle, pas au-dessus d'un dépôt Git interne.

## Conséquences

- Les opérations locales doivent rester déterministes et inspectables.
- Les objets doivent porter assez de métadonnées pour être synchronisés plus tard.
- Les conflits doivent être exprimés dans le modèle Layrs plutôt que cachés dans des fichiers temporaires.
- Les premiers tests doivent privilégier des fixtures lisibles et reproductibles.
