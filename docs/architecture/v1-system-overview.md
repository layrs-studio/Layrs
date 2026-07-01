# Vue système V1

Ce document décrit la première architecture cible de Layrs. Il ne promet pas que tous les composants existent déjà; il sert de contrat d'orientation pour les workers qui vont créer les crates, apps et packages.

## Objectifs

- Fournir un coeur local-first pour créer, relier et vérifier des états de travail.
- Modéliser directement les concepts Layrs plutôt qu'un dépôt Git interne.
- Garder un store durable et inspectable comme base commune.
- Permettre des automatisations par Steps, Flows, Gates et Policies.
- Préparer la synchronisation future sans rendre le serveur obligatoire en V1.

## Non-objectifs V1

- Compatibilité Git dans le coeur.
- Hébergement cloud obligatoire.
- Marketplace d'intégrations.
- Refonte complète des workflows CI/CD existants.

## Couches prévues

```text
Interfaces
  apps/*             UI desktop/web, API locale, surfaces d'administration.

Packages partagés
  packages/*         Types, clients et composants réutilisables côté TypeScript.

Coeur Rust
  crates/layrs-core      Modèle domaine et invariants.
  crates/layrs-store     Persistance locale, transactions et récupération.
  crates/layrs-graph     Relations entre objets Layrs et requêtes de Graph.
  crates/layrs-policy    Evaluation des Policies.
  crates/layrs-gates     Résolution des Gates et états de contrôle.
  crates/layrs-steps     Exécution et traçabilité des Steps.
  crates/layrs-weave     Fil narratif, commentaires, décisions et Proofs.
  crates/layrs-cli       Interface ligne de commande.
  crates/layrs-api       API locale ou service applicatif.
```

## Flux principal

1. Un utilisateur ou un Step ouvre un Workspace et un Space.
2. Layrs charge les métadonnées, Artifacts et relations nécessaires depuis le store local.
3. Une action crée ou modifie une Layer, une Proof, un Weave ou un Artifact.
4. Les Policies applicables sont évaluées.
5. Les Gates bloquent, acceptent ou demandent des Proofs complémentaires.
6. Le store acquitte l'écriture durablement.
7. Le Graph expose le nouvel état aux Views et Lenses.

## Store local-first

Le store doit séparer les responsabilités:

- contenu brut et Artifacts;
- métadonnées du domaine Layrs;
- index de requête;
- relations du Graph;
- journal ou traces nécessaires à la récupération.

Cette séparation doit rendre les données inspectables et limiter les pertes silencieuses. Les détails de format restent à décider dans les crates, mais les invariants produit viennent des ADRs.

## Policies et Gates

Les Policies décrivent les règles. Les Gates appliquent ces règles à un état concret.

Exemples:

- une Layer ne peut pas être promue sans Proof de test;
- une Team peut modifier un Space mais pas changer ses Policies;
- un Step automatique peut produire un Artifact mais pas l'approuver seul.

## Steps, Flows et Weaves

Un Step produit un résultat vérifiable. Un Flow ordonne plusieurs Steps. Un Weave explique le chemin: intention, changements, preuves, décisions et commentaires.

Le Weave est essentiel parce que Layrs ne veut pas seulement stocker des états; il doit rendre le raisonnement lisible.

## Synchronisation future

La synchronisation distante sera une couche au-dessus du store local-first. Elle devra transporter les objets et relations Layrs, résoudre les conflits dans les termes du domaine et respecter les Policies applicables.
