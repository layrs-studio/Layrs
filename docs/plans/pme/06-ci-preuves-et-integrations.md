# T6 — Brancher les tests et les outils existants

Suivi d'exécution : [journal T6](journal/T6.md). Coordination et reprise : [REPRISE.md](REPRISE.md).

Statut : implementation avancee, qualification partielle. Les cases ouvertes restent obligatoires dans le perimetre initial.

## Résultat attendu

Une automatisation récupère exactement la version demandée, exécute les tests hors du serveur Layrs et publie une preuve vérifiable. Une politique peut exiger cette preuve pour accepter une intégration ou publier une Release. Des webhooks permettent de raccorder les outils de la PME.

## Point de départ et références

- `crates/layrs-proof/` et `crates/layrs-gates/` contiennent déjà types et évaluation ; vérifier ce qui est réutilisable pour les produits canoniques V3.
- Les workflows `.github/workflows/` testent Layrs lui-même ; ils ne constituent pas une CI proposée aux utilisateurs de Layrs.
- Les contrats V3 incluent produits et Evidence ; les utiliser plutôt qu'inventer un second stockage de résultats.
- Autorités : [produits V3](../../contracts/sync-v3/products.schema.json), [invariants V3](../../contracts/sync-v3/invariants.md), [ADR 0012](../../decisions/0012-space-events-object-authorization.md).

## Périmètre

Inclus : identité d'automatisation limitée, récupération d'un Step exact, publication de résultat, vérification de confiance, Gates, webhooks signés/rejouables et exemple d'exécuteur externe.

Reporté : infrastructure d'exécution hébergée multi-tenant, ordonnanceur complet, marketplace d'actions, secrets de pipelines hébergés et DSL CI généraliste. Le serveur Layrs ne doit pas exécuter directement du code de dépôt.

## Plan d'exécution

### T6.C — Définir exécution, preuve et confiance

- [x] **T6.C1** Définir sujet exact : Step testé et, si nécessaire, proposition/résultat d'intégration associé ; aucun résultat lié uniquement à un nom de branche.
- [x] **T6.C2** Définir recette/version, identifiant d'exécution et de tentative, horodatages, verdict, niveau de confiance, provenance et références d'Evidence.
- [x] **T6.C3** Fixer les états d'exécution UI distincts du verdict final : en attente, en cours, terminé, annulé/expiré. Un échec et une absence de résultat ne sont pas équivalents.
- [x] **T6.C4** Mapper les données aux Proof/GateDefinition/GateEvaluation existants ; toute extension respecte les unions fermées et les identités canoniques.
- [x] **T6.C5** Définir qui peut produire quel niveau de preuve ; une déclaration du client ne suffit jamais à s'accorder un niveau de confiance supérieur.
- [x] **T6.C6** Fixer API de callback idempotente, types d'événements sortants et contrat de récupération de contenu. Publier fixtures pour T5 et l'exécuteur.

**Sortie T6.C :** contrat des preuves, callback et webhooks utilisable par serveur, UI et intégrations.

### T6.P — Persister et vérifier preuves et Gates

- [x] **T6.P1** Consommer les comptes de service/tokens révocables T8.I avec permissions bornées au Space et aux actions nécessaires.
- [x] **T6.P2** Admettre résultats et Evidence via T4.E ; valider identité, digest, taille, sujet, producteur et dépendances.
- [x] **T6.P3** Rendre le callback idempotent : doublon identique accepté, contenu contradictoire pour la même identité refusé ; une nouvelle tentative crée un nouvel objet.
- [x] **T6.P4** Évaluer la Gate selon la définition et la policy exactes : verdict, fraîcheur, recette, sujet et niveau de confiance autorisé.
- [x] **T6.P5** Exposer motifs lisibles de blocage et résultat d'évaluation traçable ; ne pas accepter une évaluation auto-proclamée par un exécuteur non autorisé.
- [x] **T6.P6** Relier T5.G et les politiques de publication T3 aux mêmes contrôles serveur ; vérifier à nouveau au moment de l'acceptation.

### T6.X — Raccorder un exécuteur externe et les webhooks

- [x] **T6.X1** Fournir une commande/API de checkout en lecture d'un Step exact, avec vérification de fermeture et de digest, dans un répertoire isolé.
- [x] **T6.X2** Refuser explicitement les états conflictuels non supportés par la recette ; ne pas construire silencieusement une variante choisie.
- [x] **T6.X3** Fournir un exemple de job externe : obtenir le Step, exécuter un test simple, publier verdict et journal borné. Les credentials restent hors du dépôt et des logs.
- [x] **T6.X4** Mettre en place une outbox transactionnelle d'événements pour ne pas perdre un webhook après l'acceptation métier.
- [x] **T6.X5** Signer les livraisons, inclure identifiant et date, gérer délai, retries bornés, déduplication côté exemple et relance manuelle avec audit.
- [x] **T6.X6** Borner les destinations selon la politique de l'instance : URL/schéma, résolution réseau, redirections et accès aux services internes. Les installations internes peuvent configurer explicitement leurs destinations autorisées.
- [x] **T6.X7** Offrir historique des livraisons et rotation des secrets sans exposer leur valeur.

## Validation et critères de fin

- [x] **T6.V1** Test externe réel : checkout de N, tests, callback puis Gate ; une modification N+1 ne réutilise pas la preuve de N.
- [x] **T6.V2** Preuve forgée, token révoqué, mauvaise recette, mauvaise identité de sujet, Evidence trop grande ou corrompue : refus explicite.
- [x] **T6.V3** Callback perdu puis rejoué, deux tentatives concurrentes, expiration et changement de policy pendant l'exécution.
- [x] **T6.V4** Webhook accepté en base puis serveur arrêté : livraison reprise au redémarrage ; signatures et limites de retry vérifiées.
- [x] **T6.V5** Destinations interdites/redirections inattendues refusées ; absence de secrets dans logs et réponses.
- [ ] **T6.V6** Une Gate imposée par le serveur bloque également l'API directe et la CLI ; affichage cohérent dans Web.
- [x] **T6.V7** Recette documentée reproductible avec un exécuteur externe simple, sans dépendre d'un service propriétaire particulier.

## Dépendances et parallélisme

| Phase | Prérequis bloquant | Parallèle possible |
|---|---|---|
| T6.C | T4.C ; accord de sujet avec T3.C | T5.C, T7.C, T8.I |
| T6.P | T6.C + T4.E + T8.I | T5.R, T3.H, T7 |
| Exemple/dispatcher isolés T6.X | T6.C | Client T4.L, UI T5, déploiement T8.H |
| T6.X complet | T6.P + T4.P | T5.G et exercices T8.B |

T5 et T6 s'accordent sur les sujets et motifs de blocage avant de travailler séparément. T6 possède Proof/Gate et livraison des webhooks ; T8 possède authentification des services et rotation de leurs tokens ; T4 possède l'admission et le journal partagé.

Charge indicative : 4–7 semaines-personnes. Risques : confusion de version testée, niveau de confiance falsifiable et livraison de webhooks. Voir [l'ordonnancement global](01-restauration-et-annulation.md#ordonnancement-global-des-huit-fichiers).

## Audit des cases - 20 septembre 2026

T6.C/P : run exact Step/recette/Principal/policy/epoch, Proof/Evidence/Gate et idempotence verifies. UI distingue en cours, expire, reussi, echoue ; attente ordonnancee et annulation explicite pas livrees.
T6.X : runner Docker externe reel, image epinglee/prechargee, isolation et journaux bornes. Webhooks collaboration signes, outbox/retries/reprise/replay/rotation et restrictions reseau verifies. Evenements SCM generiques non exposes.
Validation : native_clients_v3 PASS64s inclut runner reel et refus preuve N pourN+1 ; native_sync_v3 PASS12,37s comprend expiration, recette incorrecte, rejet contradiction et pagination101runs. Scenarios de concurrence complets et qualification visuelle Web restent ouverts. Desktop preuve/log verifies avec fixture synthetique distincte du runner reel.
Pagination runs : migration0009, curseur borne espace/epoch, ancre sequence immuable, creations intercalees exclues et verdicts actuels, API/CLI/UI raccordees. Nouvelle qualification migration et UI finale encore a executer.

Cases cochees : 25/26. Les criteres composes non integralement satisfaits restent ouverts. Voir journal pour commandes et limites.

Qualification C3 finale : migration0014, préparation/démarrage explicites, PG+backup PASS59,37s ; circuit HTTPS avec vrai runner Docker PASS263,44s ; Desktop équipe PASS51,8s montre l'attente puis l'annulation. Aucun ordonnanceur hébergé ni contrôle distant du processus n'est annoncé. V6 reste ouverte pour le parcours Web réel.

Qualification complémentaire V2 : `native_sync_v3` PostgreSQL PASS16,49s sur le code final validant Proof/Evidence depuis le CAS avant Gate. Refus des objets Proof/verdict/recette absents ou corrompus, sujet substitué, enveloppe Proof forgée par Sync, journal >256Kio, mauvaise recette et session révoquée. La View reste inchangée sur refus. Voir journal T6 pour la matrice et les bornes. V3 reste ouverte pour deux tentatives distinctes en attente simultanément ; les callbacks concurrents d’une même tentative, expiry et policy change sont déjà passés. L’annulation explicite est désormais livrée (migration0011) ; les limites indiquées dans l’audit intermédiaire ci-dessus sont historiques.
Qualification complementaire : native_sync_v3 PostgreSQL PASS20,96s ; voir dernier checkpoint du journal pour clauses cochees, preuves et limites exactes. Les mentions intermediaires de criteres encore ouverts ci-dessus sont historiques.
