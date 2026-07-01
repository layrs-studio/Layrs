# ADR 0004: Cible de durabilité du store

Date: 2026-06-29

Statut: accepté

## Contexte

Un outil de source control local-first doit inspirer confiance. Une écriture acceptée par Layrs ne doit pas disparaître silencieusement après un crash de processus, une interruption d'interface ou une reprise locale.

## Décision

La cible V1 du store est la durabilité locale après acquittement d'écriture dans les limites du système de fichiers hôte.

Le design doit privilégier:

- des écritures atomiques ou transactionnelles pour les métadonnées critiques;
- une séparation claire entre contenu brut, index et relations du Graph;
- des identifiants stables pour Artifacts, Layers, Steps et Proofs;
- des opérations de récupération capables de détecter un état partiel;
- des tests de fixtures qui simulent reprise, corruption partielle et doublons.

## Conséquences

- Les APIs d'écriture devront définir précisément le moment d'acquittement.
- Les caches ne pourront jamais être la seule source d'un objet accepté.
- Les Proofs de durabilité devront être automatisables avant une V1 publique.
- Les performances ne doivent pas être optimisées au prix d'une perte silencieuse de données.
