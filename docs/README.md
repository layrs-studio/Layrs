# Documentation Layrs

Point d’entrée mis à jour le 5 octobre 2026. Les documents ci-dessous décrivent
les sources actuelles ; les rapports datés décrivent uniquement leurs essais.

| Besoin | Document de référence |
| --- | --- |
| État du projet, prochaine étape, validations et limites | [État courant](STATUS.md) |
| Démarrer le serveur et le Web | [Serveur local](development/local-server.md) |
| Créer un projet, installer et choisir des lenses | [Guide projets et lenses](guides/projects-and-lenses.md) |
| Paquets, signatures, routes et limites des lenses | [Contrat de distribution](contracts/lens-distribution-v1.md) |
| Entreprise, organisations optionnelles, projets personnels et équipes | [Contextes des projets](contracts/project-contexts.md) |
| Utiliser captures, Layers, synchronisation, revues et Git | [Parcours natifs](native-workflows.md) |
| Développer et vérifier une modification | [Développement et tests](development/testing.md), [matrice](development/test-matrix.md) |
| Architecture du moteur | [SCM natif](architecture/scm-v1-target.md) |
| Identité visuelle et parcours attendus | [Refonte](design/redesign/README.md) |
| Couverture réellement livrée de la refonte | [Registre de livraison](design/redesign/03-delivery.md) |
| Preuves de la livraison L6, antérieures au chantier lenses | [Qualification L6](design/redesign/04-qualification-l6.md) |
| Limites du moteur et qualification de distribution | [Travail restant](plans/pme/REMAINING.md) |
| Exploiter une instance | [Auto-hébergement](../deploy/selfhost/README.md) |

Les fiches de conception sont des spécifications : une capacité décrite n’est
livrée que si le registre de livraison le confirme. Les ADR et contrats
versionnés conservent leur rôle technique ; leur numéro de version n’est pas
le statut de la livraison.

Les journaux de `plans/pme/journal/`, les mesures datées et les anciens points
de reprise sont des **archives de preuves**. Leurs réussites et leurs échecs
restent datés ; ils ne qualifient pas automatiquement les sources actuelles.
Un nouvel état doit modifier `STATUS.md` et le registre concerné, sans ajouter
une deuxième « tâche courante » sous un ancien checkpoint.
