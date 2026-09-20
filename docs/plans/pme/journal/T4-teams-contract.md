# Équipes du projet : contrat natif T4 — 20 septembre 2026

Statut : périmètre approuvé, migration additive0013 et backend/client implémentés,
compilation et Clippy strict PASS. PostgreSQL/HTTP + backup/restauration réels
PASS48,05s (419objets), DTO client2tests PASS. Voir journalT4 pour preuves exactes.

Portée strictement Space. Les membres sont des humains actifs déjà membres actifs
du Workspace ; le endpoint ne crée/réactive jamais cette appartenance. Les invitations
et droits individuels restent explicites. Aucun Team Workspace ni nouveau ProductKind.
Mutation : droit policy.administer ET rôle Workspace owner/administrator.

API sous /v3/spaces/:space/teams : GET liste, POST création expectedVersion0,
GET /eligible-members, PUT /:team remplacement versionné,
POST /:team/archive avec reason, GET /:team/history?after=0&limit=50&anchor=N.
Corps changement : {idempotencyKey,expectedVersion,name,role,memberPrincipalIds}.
Rôles fermés : member, auditor, release_manager. Réponse mutation :
{team:{teamId,spaceId,version,name,role,memberPrincipalIds,state,eventObjectId,
previousEventObjectId,policyProductObjectId,previousPolicySetId,policySetId,
authorizationEpoch,changedBy,createdAt,updatedAt,reason},policySetId,authorizationEpoch}.
State active/archived. Liste {teams,policySetId,authorizationEpoch} ; éligibles
{members:[{principalId,displayName}]} ; historique {events:[{eventObjectId,record}],
anchorVersion,nextAfter,hasMore}. Identifiants team: suivis de64hex, jamais noms.

Bornes :32 équipes (archives comprises),64 membres/équipe,128octets nom,
1000 versions,32 Policies au total (grants individuels et politiques existantes
compris),100 événements/page et1MiB agrégé,256 choix éligibles. Au-delà refus
explicite, pas de résultat tronqué. Aucune nouvelle limite globale Workspace
prétendument imposée par cette fonctionnalité.

Journal immuable Evidence layrs.space-team.v1 lié au précédent et aux policies.
Projection teams/members, Policy Product et PolicySet/epoch changent atomiquement
sous verrous Space/identités. PolicySet reste autorité : allow expansé en PrincipalIds,
deny individuel prioritaire ; retirer un grant ne retire pas les grants indépendants.
Équipe vide ou archivée ne conserve aucune règle de grant. Pas de désarchivage.
Retry identique retrouve événement original ; même clé autre payload ou version
périmée refusée. Concurrence sur epoch peut nécessiter retry après409.
Le rôle SQL runtime ne peut UPDATE/DELETE le journal. CAS déjà écrit avant rollback
peut contenir des objets orphelins, jamais des droits partiellement admis.
Notifications team.changed sans contexte de revue ; aucun secret dans les événements.

Validation finale : PG/HTTP et backup réel PASS48,05s ; identité/services/membres
révoqués, droits/capacités/cursors, CAS concurrent, audit, limites/rollback,
reconstruction projection et autorisations effectives après restauration couverts.
Clippy ciblé strict PASS, client DTO2tests PASS. Migration0013 inspectée, checksum
verrouillé, native_migrations PASS3,50s par operations. Root qualifie CLI/Desktop
séparément ; aucune recette visuelle Web n'est revendiquée par ce lot serveur.
Prochaine fonctionnalité T4 : choose/Decision selon invariant47, contrat à préciser.