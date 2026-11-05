# T5 — Faire une revue d'équipe et protéger les intégrations

Suivi d'exécution : [journal T5](journal/T5.md). Coordination et reprise : [REPRISE.md](REPRISE.md).

Statut au 22 septembre 2026 : 27/27 cases vérifiées dans le périmètre initial. La collaboration autonome des comptes à contenu restreint reste une extension Store3 non livrée ; voir [PARTIAL-FOLLOWUP.md](PARTIAL-FOLLOWUP.md).

## Résultat attendu

Un membre propose un changement, ses collègues commentent une version précise, demandent des corrections ou approuvent, puis intègrent seulement si les droits et conditions sont encore satisfaits. Les retours et leur contexte restent consultables après de nouvelles captures.

## Point de départ et références

- Weaves, demande d'application et résolution existent dans l'ancienne pile `crates/layrs-server/src/web/sections/weave_*`.
- Desktop expose déjà une vue de fusion ; Web a des composants de Layers/Steps et des types de présentation. Ne pas compter les fixtures UI comme un workflow de revue réel.
- Le nouveau modèle sépare résultat de fusion, produits métier, décisions, policies et preuves. Un fil de revue ne devient pas une session de conflit faisant autorité.
- Autorités : [ADR 0010](../../decisions/0010-weaves-and-lens-reconciliation.md), [ADR 0011](../../decisions/0011-sync-v3-multi-head-transactions.md), [produits V3](../../contracts/sync-v3/products.schema.json).

## Périmètre

Inclus : proposition, participants, commentaires globaux/sur diff, réponses, corrections, approbations, notifications dans l'app, protection de Layer et intégration conditionnelle.

Reporté : tickets/projets complets, chat, règles organisationnelles complexes, notifications email obligatoires et marketplace. Une équipe doit pouvoir utiliser la revue sans disposer d'un fournisseur de messagerie.

## Plan d'exécution

### T5.C — Définir la revue et ses versions

- [x] **T5.C1** Distinguer identité stable de la revue, version proposée, têtes source/cible exactes, résultat Weave éventuel et décisions immuables.
- [x] **T5.C2** Mapper ces données aux produits V3 existants. Tout nouveau type ou événement doit passer par une extension explicite du schéma fermé, avec fixtures et validation Rust/TS.
- [x] **T5.C3** Définir états et transitions : brouillon, ouverte, corrections demandées, approuvée, intégrée, fermée. Les agrégats UI sont des projections reconstructibles.
- [x] **T5.C4** Lier commentaire de diff à une version exacte, un chemin et un ancrage borné. Après modification, conserver l'ancien contexte et marquer l'ancrage obsolète ; ne pas le déplacer silencieusement.
- [x] **T5.C5** Lier approbation à un digest du contenu/proposition et des têtes pertinentes. Définir les invalidations lorsque source, cible ou politique change.
- [x] **T5.C6** Définir droits de lecture/commentaire/approbation/intégration et la politique d'auto-approbation, explicite et configurable.
- [x] **T5.C7** Fournir le contrat des protections à T6 : conditions exigées, sujet exact des preuves, acteur autorisé et motifs de blocage.

**Sortie T5.C :** contrats de revue, d'ancrage et d'approbation stables. L'UI peut être développée sur fixtures identiques à celles du serveur.

### T5.R — Livrer le workflow sur les services réels

- [x] **T5.R1** Persister commentaires, réponses, corrections et décisions comme données traçables ; réviser un commentaire sans effacer son historique d'audit selon la politique documentée.
- [x] **T5.R2** Construire création/liste/détail de revue, sélection des participants et consultation d'un diff exact via T2.
- [x] **T5.R3** Implémenter autorisation à chaque lecture et écriture ; ne pas exposer extraits, chemins ou notifications à un participant qui a perdu l'accès.
- [x] **T5.R4** Fournir boîte de notifications dans l'app avec marquage lu, événements dédupliqués et lien vers la bonne version.
- [x] **T5.R5** Implémenter changements demandés puis nouvelle version proposée ; montrer clairement quelles approbations sont encore valides.
- [x] **T5.R6** Brancher les écrans Web et les liens Desktop ; les écritures passent par les APIs métier, sans logique de contrôle d'accès dans le seul navigateur.

### T5.G — Appliquer les protections sans course

- [x] **T5.G1** Définir une protection minimale : droit d'intégration, nombre d'approbations, décisions bloquantes, preuves requises et politique sur les conflits non résolus.
- [x] **T5.G2** Réévaluer côté serveur les têtes et policies exactes au moment d'intégrer ; verrouillage/comparaison fournis par T4.
- [x] **T5.G3** Vérifier que les preuves T6 concernent le résultat exact à accepter, pas une ancienne tête ni seulement le nom de la Layer.
- [x] **T5.G4** En cas de cible avancée, recalculer le résultat et invalider les éléments devenus périmés ; ne pas présenter une ancienne approbation comme permission universelle.
- [x] **T5.G5** Faire respecter ces protections via CLI, API directe et Web, y compris contre un publish qui tenterait de contourner le bouton de revue.
- [x] **T5.G6** Si un contournement administrateur est autorisé par politique, exiger une action explicite, un motif et un événement auditable.

## Validation et critères de fin

- [x] **T5.V1** Revue de bout en bout avec deux comptes : proposer, commenter, corriger, approuver et intégrer.
- [x] **T5.V2** Commentaire conservé sur sa version après déplacement ou suppression de lignes ; aucun rattachement silencieusement erroné.
- [x] **T5.V3** Deux intégrations simultanées : l'une ne valide pas une cible obsolète en écrasant l'autre.
- [x] **T5.V4** Approbation sur version N invalide pour N+1 selon les règles ; droits retirés et proofs périmées bloquent au serveur.
- [x] **T5.V5** Tests d'accès direct aux APIs : utilisateur non membre, lien connu, changement de droits entre lecture et application.
- [x] **T5.V6** Notifications sans doublons sur reprise et sans fuite de contenu après révocation.
- [x] **T5.V7** Une Layer protégée refuse le même contournement depuis chaque surface.
- [x] **T5.V8** Parcours Web réel et CLI contre le serveur V3 ; tests de composants seuls insuffisants. Recette commune22sept PASS33,4s : vraie revue Web, commentaire écrit dans Web et relu par CLI sur la même instance HTTPS, références partagées et refus de Gate visible. Les comptes restreints restent refusés sur la collaboration non filtrée.

## Dépendances et parallélisme

| Phase | Prérequis bloquant | Parallèle possible |
|---|---|---|
| T5.C | T2.C + T4.C | T6.C, T3.H, T7.C |
| UI et modèle T5.R isolés | T5.C | T4.L, T8.H ; fixtures explicites |
| T5.R intégré | T5.C + T2.M + T4.E + T8.I | T6.P, T7 et T1.R |
| T5.G | T5.R + T6.P + T4.P | T8.B et recettes Git T7 |

La revue de base n'attend pas le runner CI : les tests/contrats de Gate peuvent être préparés, mais une protection exigeant des preuves réelles n'est terminée qu'après T6.P. T5 possède revues/commentaires/notifications ; T6 possède preuves/évaluation ; T4 possède acceptation transactionnelle et autorisation.

Charge indicative : 5–8 semaines-personnes. Risques : approbations périmées et ancrages de commentaires. Voir [l'ordonnancement global](01-restauration-et-annulation.md#ordonnancement-global-des-huit-fichiers).

## Audit des cases - 20 septembre 2026

T5.C : contrats Evidence, versions exactes, ancrages et decisions verifies dans native_sync_v3. Brouillons locaux de commentaires en finition ; aucun brouillon de proposition serveur.
T5.R : propositions/participants/diff, commentaires/reponses et autorisations livres ; correction par nouvelle reponse conserve le texte original, edition auditee distincte non livree. Notifications lues/deduplication presentes, lien vers version exacte encore en cours.
T5.G : contraintes reevaluees dans la transaction et preuve d'integration immuable. Aucun bypass administrateur : le critere conditionnel ne requiert donc pas de route de contournement. Cible avancee refusee ; recalcul Weave explicite, pas de recalcul automatique depuis la revue.
Validation : native_sync_v3 PostgreSQL reel PASS12,37s, native_clients_v3 TLS/Docker/backup PASS64s, CLI distante16commandes PASS6,61s, Desktop equipe PASS23,8s avant derniers brouillons. Web reel non execute localement (outil navigateur indisponible), gate CI en construction.

Cases cochees : 26/27. Les criteres composes non integralement satisfaits restent ouverts. Voir journal pour commandes et limites.

Qualification complementaire : native_sync_v3 PostgreSQL PASS20,96s ; voir dernier checkpoint du journal pour clauses cochees, preuves et limites exactes. Les mentions intermediaires de criteres encore ouverts ci-dessus sont historiques.
