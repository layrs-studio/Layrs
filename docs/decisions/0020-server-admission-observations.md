# ADR 0020 — Observations de première admission serveur

Statut : accepté, 20 septembre 2026. Complément aux ADR0006 et ADR0017 ; identité des Steps inchangée.

Complément ultérieur : [ADR0021](0021-authenticated-client-observations.md) ajoute
les assertions clientes avec provenance distincte. Les paragraphes ci-dessous
décrivent la première livraison et restent exacts pour `server_admission` ; le
refus global des observations clientes est remplacé par le profil strict0021.

## Provenance explicite

Le serveur enregistre une `StepObservation` lorsque le Step devient admis pour la première fois dans un Space. `captureReason=received`, ActorId issu du Principal TLS authentifié, `observedAt` provenant de l'horloge serveur lors de la préparation d'admission, normalisé à la microseconde PostgreSQL. Aucun OperationId, WorktreeId ou DeviceId local n'est copié. Ce n'est ni la date de création du Step ni une attribution d'auteur du contenu. L'interface indique « réception serveur ».

La provenance canonique contient `server_admission`, PrincipalId et digest de la demande. La migration0015 lie l'observation à sa première admission par un registre append-only et une empreinte du record JCS. L'ordinal est propre au Space, attribué sous son verrou : il ne divulgue aucune activité d'autres projets. Les identités locales déclarées restent locales ; les observations fournies par les clients restent refusées. Aucun rattrapage ne fabrique des observations pour des Steps déjà admis avant cette migration.

Insertion du Step, observation, provenance, événement et reçu partagent la transaction SQL avec revalidation des droits. Deux admissions concurrentes du même Step sans mutation View produisent une seule observation. Un rejeu exact ne modifie ni identité, ni date, ni ordinal. `acceptedObservationIds` acquitte uniquement des observations fournies par le client et reste vide dans cette portée. Les observations générées sont annoncées comme effet serveur dans `StepsAccepted.payload.observationIds` et lues par l'historique.

## Lecture et filtres

L'historique renvoie les Steps canoniques et leurs observations séparément, avec provenance, PrincipalId, réception serveur et digest de demande. ActorId et bornes RFC3339 inclusives sélectionnent une même observation ; période signifie réception serveur. Un Step sans observation est visible sans filtre et ne correspond pas à un filtre d'acteur/période.

Une lecture PostgreSQL REPEATABLE READ reconstruit l'index depuis les records validés et leur provenance. Population et octets sont bornés avant allocation : 100000 records, 64 Mio et 4096 octets par record. Le curseur v2 conserve `{spaceId,maxOrdinal,count,digest}` et le filtre exact, en plus de la racine Step, du chemin, du Principal et de l'authorization epoch. Les append ultérieurs ne changent pas ses pages ; record modifié, absent ou renuméroté invalide le snapshot. Les curseurs v1 doivent être recommencés.

L'index interne n'est pas une autorisation : chaque Step et chemin retourné reste autorisé, puis l'identité et l'epoch sont revalidées avant réponse. Les droits actuels portent sur le Space entier ; cet index ne promet pas des ACL partielles par chemin. Aucune observation locale ne devient automatiquement une identité distante. Sauvegarde PostgreSQL et restauration conservent les records, provenance et ordinaux ; la lecture reconstruite doit produire le même snapshot.
