# ADR 0003: Terminologie Workspace, Team et Space

Date: 2026-06-29

Statut: accepté

## Contexte

Layrs a besoin d'une terminologie stable pour éviter de mélanger organisation, droits, dépôt, projet et espace de travail local. Les termes empruntés à GitHub ou Git peuvent aider à expliquer le produit, mais ne doivent pas piloter le modèle.

## Décision

La V1 retient:

- Workspace pour le périmètre d'organisation.
- Team pour les groupes de membres et d'accès.
- Space pour l'unité de travail comparable à un repo ou projet.

Ces termes sont les noms canoniques dans les docs et les futures APIs.

## Conséquences

- Le README et le glossaire utilisent ces termes en premier.
- Les autres termes comme organisation, repo ou projet restent explicatifs seulement.
- Les Policies peuvent viser Workspace, Team ou Space selon leur portée.
- Les futures interfaces doivent éviter de renommer Space en repository dans le coeur produit.
