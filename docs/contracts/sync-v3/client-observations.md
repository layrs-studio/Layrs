# Assertions d'observation clientes — profil natif V3

Le serveur accepte `SyncTransactionRequest.observations` après la migration
additive0016. Il s'agit de déclarations authentifiées, jamais d'une preuve de
création du contenu. Les Steps restent sans auteur ou date intrinsèques.

Construire le record avec `layrs_sync::v3::client_observation(space, principal,
actor, step, observed_at, reason)`. Le helper `client_observation_id` expose le
calcul déterministe décrit dans [ADR0021](../../decisions/0021-authenticated-client-observations.md).
Les enums de capture utilisent les valeurs SCM existantes, par exemple
`manual_snapshot`. Le serveur recontrôle indépendamment chaque champ.

Conditions d'admission :

- Space exactement égal au Space de la transaction.
- ActorId exactement égal à celui du principal authentifié ; aucune identité
  locale nouvelle n'est enregistrée par cet appel.
- Step déjà admis dans ce Space, ou entièrement validé dans le même batch.
- OperationId absent, WorktreeId et DeviceId nuls ; metadata vide.
- ObservationId recalculé sur le domaine dédié et les déclarations exactes.
- Au plus10 000 assertions par requête et par principal/Space ;100 000 par
  Space ;256 par Step tous principaux confondus ;4096 octets par record et
 64Mio de records d'assertions par Space. Ces quotas concernent les nouvelles
  identités ; une republication identique ne consomme pas de place supplémentaire.

La revalidation finale d'identité, des droits et de l'epoch précède les effets.
Insertion du Step, assertion, provenance, événements et reçu partage la même
transaction SQL. Un conflit d'identité, une assertion invalide ou un quota atteint
rejette le batch entier. Aucun overwrite et aucune suppression d'histoire.
`acceptedObservationIds` contient les assertions demandées acceptées, triées ;
les observations de première admission générées par le serveur restent à part.
Un événement `steps.accepted` peut avoir `stepIds=[]` et seulement les nouvelles
`observationIds`. Une republication sous une autre clé garde la première
provenance et ne crée pas de nouvel événement pour l'assertion déjà présente.

L'historique renvoie `provenance: "client_assertion"`, le principal authentifié,
le digest de la demande d'origine et `receivedAt` serveur, en plus du record
client dont `observedAt` conserve sa précision et son fuseau déclarés. Les
filtres de période existants portent exclusivement sur `receivedAt`. Une date
cliente peut être ancienne ou future ; elle ne certifie pas une chronologie.

`observationSnapshot.version=2` est obligatoire : le digest lie également source,
principal, date reçue et demande. Les anciens curseurs dépourvus de cette version
sont refusés et doivent recommencer. Les ajouts ultérieurs, même antidatés et
portant sur un ancien Step, restent hors d'un snapshot déjà ouvert. Les pages
s'arrêtent aussi avant le budget2Mio, entre deux Steps complets ; le curseur
reprend le premier Step différé sans tronquer ses observations.

Compatibilité : records StepObservation v1, baseline SQL native et migration0015
inchangés. La colonne de provenance et l'unicité partielle sont dans0016 ; anciens
binaires ne doivent pas écrire une base migrée sans suivre la vérification de
version de l'installation. Le backup PostgreSQL conserve les deux provenances,
les dates et les ordinaux ; la lecture restaurée revalide le snapshot exact.
