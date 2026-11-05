# ADR 0021 — Assertions d'observation clientes authentifiées

Statut : accepté pour implémentation, 20 septembre 2026. Complément aux ADR0017
et0020 ; Step et StepObservation v1 conservent leur identité et leur schéma.

Une assertion cliente ne prouve ni auteur du contenu ni date de capture. Elle
exprime qu'un principal authentifié déclare avoir observé un Step à une date.
Le serveur exige le Space exact, un Step admis dans ce Space ou validé dans le
même batch, ActorId égal à celui du principal authentifié, aucune Operation,
Worktree ou Device et metadata vide. Les identités locales déclaratives ne sont
pas importées. `observedAt` et `captureReason` sont des déclarations clientes ;
`receivedAt` vient du serveur et reste la date des filtres d'historique existants.

L'ObservationId est `observation:` suivi du BLAKE3 du tuple JCS
`["layrs.client-step-observation.v1",SpaceId,PrincipalId,StepId,observedAt,captureReason]`.
Le serveur le recalcule. Une republication identique ne crée ni nouvel ordinal
ni nouvelle provenance. Une collision avec des octets ou une autorité différents
rejette toute la transaction. Un rejeu de requestDigest/idempotencyKey conserve
son reçu exact. Aucun record antérieur n'est réécrit.

La migration0016 ajoute `client_assertion` à l'extension de provenance, conserve
les observations `server_admission` existantes et leur unicité par Step via un
index partiel. Les deux sources partagent l'ordinal par Space sous verrou.
`acceptedObservationIds` acquitte uniquement les assertions demandées ; les
premières admissions générées serveur restent des effets distincts. Un événement
StepsAccepted peut annoncer uniquement des observations (`stepIds=[]`).

Lecture reconstruite : snapshot version2, digest liant records, source,
principal, requestDigest, date reçue et ordinal. Les anciens snapshots sont
explicitement refusés. Aucune date antidatée ne réintègre un snapshot antérieur.
Les contrôles d'accès Step/chemin et la revalidation d'epoch restent obligatoires.

Bornes initiales :10000 assertions par requête (contrat existant),4096 octets
par record,10000 assertions par principal et Space,100000 assertions par Space,
64Mio de records d'assertions par Space et256 assertions par Step tous principaux
confondus (un seul Step ne doit pas dépasser la borne de sa page d'historique).
Refus sous verrou avant effets métier.
Ces bornes ne changent pas les limites de lecture de l'historique ; un historique
dépassant sa borne de lecture continue d'être refusé explicitement.

La sauvegarde/restauration conserve la provenance, les timestamps et les
ordinaux exacts. UI et clients doivent afficher « déclaration cliente » et
« réception serveur » séparément ; aucune assertion distante n'est convertie
silencieusement en Capture locale.
