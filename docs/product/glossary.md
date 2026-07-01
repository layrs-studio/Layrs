# Glossaire produit Layrs

Ce glossaire fixe les termes de base de la V1. Les définitions doivent rester courtes, opérables et cohérentes entre produit, architecture et code.

## Workspace

Un Workspace est le périmètre d'organisation principal. Il porte l'identité, les Teams, les Spaces, les Policies globales et les réglages de gouvernance.

## Team

Une Team est un groupe de membres dans un Workspace. Elle sert à attribuer des droits, des responsabilités, des Gates et des Policies sans lier ces règles à des personnes isolées.

## Space

Un Space est l'unité de travail principale, proche du rôle produit d'un repo ou d'un projet, mais sans modèle Git dans le coeur V1. Il contient des Layers, des Artifacts, des Weaves et leur Graph.

## Layer

Un Layer est un état nommé, traçable et composable d'un Space. Il peut représenter une base stable, une exploration, une proposition ou un résultat automatisé.

## View

Une View est une projection lisible d'un Space, d'un Layer ou du Graph. Elle peut filtrer par fichier, domaine, décision, Step, Artifact, Gate ou Policy.

## Artifact

Un Artifact est un résultat stocké et référencé par Layrs. Il peut être un fichier, une note, une image, un rapport, une preuve de test, une sortie de Step ou une capture de décision.

## Step

Un Step est une action automatisée ou semi-automatisée qui lit un état et produit un changement, un Artifact, une Proof ou un signal de Gate.

## Flow

Un Flow est une suite ordonnée de Steps. Il décrit une procédure réutilisable, par exemple analyser un Space, produire une Layer candidate, exécuter des Gates puis publier des Artifacts.

## Weave

Un Weave est le fil narratif qui relie intentions, changements, décisions, commentaires, Proofs et Artifacts. Il explique pourquoi un état existe, pas seulement ce qui a changé.

## Lens

Une Lens est une perspective spécialisée sur le Graph. Elle sert à isoler un angle d'analyse: sécurité, produit, dette technique, dépendances, ownership, qualité ou livraison.

## Graph

Le Graph relie les objets Layrs: Workspaces, Teams, Spaces, Layers, Artifacts, Steps, Flows, Weaves, Lenses, Proofs, Gates et Policies. Il devient la carte de vérité locale et synchronisable.

## Proof

Une Proof est une évidence attachée à une décision, une Gate, un Step ou un Artifact. Elle peut être automatique ou humaine, mais doit être assez précise pour être vérifiée plus tard.

## Gate

Une Gate est un point de contrôle. Elle autorise, bloque, demande une Proof ou déclenche une action selon les Policies et le contexte du Space.

## Policy

Une Policy est une règle déclarative qui gouverne les droits, les Gates, les Steps permis et les comportements attendus dans un Workspace, une Team ou un Space.
