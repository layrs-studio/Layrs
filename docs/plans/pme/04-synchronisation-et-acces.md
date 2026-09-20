# T4 — Travailler à plusieurs sans perdre de données

Suivi d'exécution : [journal T4](journal/T4.md). Coordination et reprise : [REPRISE.md](REPRISE.md).

Statut au 20 septembre 2026 : service V3 et synchronisation native livrés pour le périmètre vérifié ; T4 reste partiel. Priorité : P0. Les cases composées restent ouvertes dès qu'un critère manque. Voir le journal pour les recettes PostgreSQL/HTTPS réelles.

Livré : provisioning Workspace/Space, authentification réelle, règles explicites par Principal, équipes du projet, changements de droits/epoch atomiques, `advance`/`integrate`/`choose`, mutations PolicyRoot v1/v2 avec enforcement réel et lectures de contenu autorisées par chemin, reprise persistée, conflits échangés entre deux stores, références immuables et annulation coopérative clone/réception/publication/candidats (Desktop et Ctrl+C CLI). Choose et PolicyRoot exigent policy.administer et un rôle Workspace owner/administrator sous l'ancienne autorité ; attentes exactes et aucune suppression d'histoire. Les politiques personnalisées ne sont jamais remplacées silencieusement par les APIs gérées. Les Observations clientes authentifiées sont qualifiées avec provenance, dates distinctes, quotas et admission atomique. Les flux publication/clone/réception/restauration100Mio/1Gio et les redémarrages serveur/client sont qualifiés dans leurs bornes documentées. Restent notamment le Worktree à projection partielle, les révisions Layer et le parcours navigateur Web complet ; les lectures fines autorisées ne constituent pas un Worktree partiel éditable.

**Invitations nominatives et équipes du projet livrées.** La migration 0010 couvre invitation/acception/refus/révocation ; consommation concurrente unique, grant/PolicySet/epoch atomiques et absence de réattribution par rejeu sont vérifiés avec PostgreSQL réel. L'inbox fonctionne sans projet ouvert. La migration 0013 ajoute des équipes propres au Space (32 équipes, 64 membres chacune), uniquement des humains actifs déjà membres actifs du Workspace ; aucun accès Workspace implicite ni réactivation d'appartenance inactive. Les mutations attendent la version exacte, installent une Evidence immuable et modifient PolicySet/epoch atomiquement. Les capacités et curseurs périmés sont refusés ; reconstruction et backup/restore sont vérifiés. API/client, CLI et Desktop sont qualifiés, dont parcours Teams Desktop PASS 37,8 s. La recette visuelle finale des invitations reste distincte. Voir [le contrat opérateur](../../../deploy/selfhost/INVITATIONS.md) et le journal T4.

## Résultat attendu

Deux clients connectés au vrai serveur échangent un projet, travaillent hors ligne, publient simultanément et récupèrent les divergences sans écrasement. Une coupure ou une réponse perdue peut être reprise. Les droits sont vérifiés à chaque frontière serveur.

## Point de départ historique au 19 septembre et références

- `crates/layrs-server/src/main.rs` monte V3 avec `UnavailableV3Facade`. Seules cette façade et une façade de test implémentent actuellement `V3Facade`.
- Les clients appellent encore V1 ; `http_client.rs` n'accepte que HTTP.
- Contrats V3, routeur, transferts et primitives PostgreSQL/CAS existent. Ils ne constituent pas encore le service transactionnel intégré.
- Autorités : [ADR 0011](../../decisions/0011-sync-v3-multi-head-transactions.md), [ADR 0012](../../decisions/0012-space-events-object-authorization.md), [invariants V3](../../contracts/sync-v3/invariants.md).

## Périmètre et frontières

T4 possède protocole, transaction serveur, sync client, autorisation et gestion Workspace/Team/Space/Layer. T8 possède comptes, sessions, récupération, bootstrap administrateur et configuration du service. Les identités techniques CI sont fournies par T8, leurs usages et preuves par T6.

Les Operations et chemins Worktree restent locaux. Les Views publiées n'embarquent aucun état de checkout. Il n'y a ni compatibilité V1/V2 ni conversion automatique de stores de développement.

## Plan d'exécution

### T4.C — Fixer les interfaces de service

- [x] Relire toutes les routes V3 et identifier pour chacune les primitives déjà disponibles et l'implémentation manquante.
- [x] Définir façade métier, injection du repository PostgreSQL, du CAS, du vérificateur d'identité T8.C et de l'authorizer ; séparer ces interfaces du routeur HTTP.
- [x] Fixer attentes de View/génération, erreurs transport versus refus métier, reçus persistés, reprise et sérialisation canonique.
- [x] Définir les APIs de gestion d'équipe et leurs effets sur PolicySet/authorization epoch ; ne pas réactiver les anciennes routes pour contourner la refonte.
- [x] Définir l'API cliente commune pour bootstrap, plan, upload, transaction, événements et fetch ; fournir fixtures SDK/serveur concordantes.

**Sortie T4.C :** interfaces serveur/client et contrat d'identité partagé avec T8 ; T5/T6 peuvent construire leurs consommateurs.

### T4.E — Implémenter le vrai moteur transactionnel

- [x] Construire une façade réelle autour de `PgV3Repository`, du CAS sélectionné et de l'autorisation. Les doubles d'identité restent limités aux tests.
- [x] Vérifier identités canoniques, types, appartenance au Space, fermeture des parents et objets, et reconstruction des FileIds.
- [x] Installer les objets avant les références ; prendre une transaction courte avec le snapshot d'autorisation de départ.
- [x] Implémenter `advance`, `integrate` et `choose` selon les contrats : comparaison exacte, union normalisée ou décision autorisée explicite. Ne pas modifier une table de têtes comme autorité parallèle.
- [x] Accepter atomiquement objets métier, View, racines de politiques, événements, AuthorizationSnapshot, reçu et idempotency record selon les invariants V3. Products/Steps/View/PolicyRoot/événements/reçus et snapshot partagent la transaction PostgreSQL ; changements de droits Teams/grants atomiques. PolicyRoot v1 qualifié PG+backup et client HTTPS : ancienne autorité, CAS, refus adversariaux, objet inerte avant activation, enforcement et rejeu exact. Assertions clientes désormais qualifiées séparément : acteur authentifié, Step admis dans ce Space ou même batch, quotas, dates déclarée/reçue distinctes, corruption et course epoch refusées ; PG+backup PASS88,39s, CLI60commandes et Desktop réels. Les seules assertions demandées sont acquittées ; aucune Operation/Worktree/Device locale n'est transférée. [ADR0021](../../decisions/0021-authenticated-client-observations.md), [ADR0022](../../decisions/0022-policy-root-v1-and-managed-policy-provenance.md).
- [x] Rejouer un reçu identique pour une même demande ; refuser la même clé avec un digest différent ; préserver les reçus de refus métier.
- [x] Finaliser bootstrap, plans de transfert, pagination d'événements, téléchargements reprenables et expiration/révocation des capacités dans le sous-ensemble borné. Vraie façade, pagination serveur, capacités signées identité/epoch/expiration, révocation réelle Teams et reprise Range qualifiées. Défaut de persistance FetchPlan corrigé : snapshot complet immuable avant pages, deux pages/rejeu/octets exacts et changement d'epoch testés en HTTPS ; snapshot total10k objets. Le client natif demande un claim indivisible par plan et n'a pas besoin de boucle `has_more` pour compléter sa fermeture. Spool Identity après redémarrage et renouvellement exact après expiration chronologique en cours d'objet qualifiés séparément en HTTPS ; deux renouvellements/objet maximum, deadline120s inchangée, autorité et fermeture exactes exigées. Zstd durable reste hors sous-ensemble. [Contrat de pagination](../../contracts/sync-v3/fetch-pagination.md), [reprise](../../contracts/sync-v3/durable-downloads.md).
- [x] Raccorder membres/équipes/invitations/droits à la nouvelle baseline et invalider les capacités/cursors lors d'un changement d'autorisation.

**Sortie T4.E :** service réel vérifié avec PostgreSQL et CAS isolés, y compris courses concurrentes. Le transport d'authentification de production attend T8.I.

### T4.L — Remplacer le parcours client

Conflits riches mono-placement qualifies : publication/reception de blocs texte/code WASM, fermeture des fragments autorisee, origines/ancres verifiees, resolution partielle persistante puis finale entre deux Stores HTTPS. Receipts rehashes host/pins/roles faux, payload falsifie et retour sur une decision publiee refuses. Recette complete avec runner Docker et restauration **PASS263,44s** ; [contrat et limites](../../../deploy/selfhost/NATIVE-CONFLICTS.md). Les renommages multi-placements exacts sont désormais également qualifiés : fermeture/ancres/origines/DAG et lignée validés, choix couplé transporté entre deux Stores, falsifications refusées. Les autres modules historiques restent refusés ; aucune attestation d'exécution distante n'est revendiquée.

Téléchargements qualifiés le 20 septembre : reprise HTTP Range après coupure, spool privé, framing/digest vérifiés et budget de tentatives borné ; fichier 16 Mio avec chunks répétés et hash final testé sur HTTPS/PostgreSQL réels. [Mesure](measurements/T4-transfers-2026-09-20.json). Annulation coopérative clone/receive qualifiée en HTTPS : interruption avant commit sans mutation locale, checkout propre exigé, CAS atomique contre le début de commit, nouvel essai exact ; aucune capture implicite ni verrou local pendant le réseau. Publication et candidats annulables sont également qualifiés en HTTPS (recette292,93s) : checkout propre, uploads interrompus avant journal, commit atomique trop tardif, réponse perdue/retry exact et aucune fausse annulation du pending. CLI Ctrl+C portable raccordée, helper Windows testé, vraie SIGINT Linux qualifiée ; Desktop équipe PASS~1,1min incluant publication annulée/nouvel essai et TooLate reçu appliqué. Ces mesures initiales sont complétées par les recettes HTTPS100Mio/1Gio avec capture, clone, réception incrémentale et restauration exactes, documentées dans les mesures T4 de reprise. Le spool Identity durable après redémarrage est désormais qualifié séparément (recette HTTPS avec nouveau processus OS, voir journal et contrat durable-downloads).

- [x] Remplacer le client HTTP artisanal par un transport HTTPS avec validation des certificats, timeouts, annulation et bornes de téléchargement. Parcours natif `reqwest` HTTPS strict/CA explicite, redirections interdites, connexion15s/requête120s (30s contrôlée), spool et framing bornés ; annulation des quatre transferts qualifiée API/Desktop et handler Ctrl+C Linux réel. Les anciens parcours non natifs ne sont pas réactivés. Le code de transport appartient à T4 ; terminaison TLS et certificats appartiennent à T8.
- [x] Envoyer seulement les objets manquants puis la transaction ; conserver les objets et intentions tant que le reçu exact n'est pas vérifié.
- [x] Persister les étapes de reprise et la clé d'idempotence avant les effets réseau ; reprendre après redémarrage sans nouveau changement métier.
- [x] Télécharger la fermeture autorisée, vérifier les octets puis installer localement via T1.S ; ne déplacer aucune référence avant disponibilité des objets.
- [x] Transformer l'état reçu en Operation locale ; conserver changements non publiés et têtes concurrentes. Une réponse ancienne ne remplace pas l'état courant.
- [ ] Ne pas remplacer/supprimer des fichiers masqués par les droits à partir d'une vue partielle ; définir le traitement visible des chemins réservés. **Acquis :** lectures fines de contenu autorisées par chemin avec PolicyRoot v2, refus de fermeture incomplète avant installation d'un Store complet ; une interdiction n'est jamais interprétée comme une suppression. **Manque :** modèle et recette de Worktree à projection partielle avec ACL par chemin ; le contrat actuel ne délivre pas cette fonctionnalité.
- [x] Brancher CLI, Desktop et SDK/Web sur le service V3 ; rendre incompatibilités et actions de reprise compréhensibles.

### T4.P — Prouver le parcours complet

- [x] Utiliser le binaire serveur normal, sa vraie façade et l'identité T8.I, sur l'installation HTTPS T8.H.
- [ ] Relier deux stores clients distincts ; publier/recevoir avec la CLI puis avec Desktop/Web. **Acquis :** deux Stores API HTTPS réels, CLI distante et parcours Desktop qualifiés séparément. **Manque :** parcours navigateur Web complet équivalent et recette combinée de toutes les surfaces ; un build Web ne le prouve pas.
- [x] Couper la connexion pendant objets, transaction et retour du reçu ; redémarrer serveur/client puis reprendre. **Parcours Identity borné qualifié :** coupure objet avec reprise Range, spool privé persistant repris dans un nouveau processus client, refus corruption/ancienne autorité et renouvellement exact après expiration ; clients OS tués avant dispatch et après commit avant reçu. Serveur réellement tué puis redémarré sur mêmes PG/CAS/clé/origine HTTPS aux trois frontières : objet GET Range après préfixe durable effectivement observé, avant dispatch transaction, après commit avant réponse. Nouveau plan/capacité, même claim/objet/principal/Space/epoch/offset, reconstruction finale exacte sans Store avant fermeture ; journal et reçu exacts sans progression supplémentaire de View. Recette des trois frontières Windows **PASS20,87s**, Clippy strict PASS11,58s. Enfant de test utilisant le vrai routeur et rôle non privilégié, aucun service simulé. [Contrat de reprise](../../contracts/sync-v3/durable-downloads.md), journal T4 pour commandes/preuves. **Limites distinctes :** pas de reprise persistante zstd, pas de crash PostgreSQL au milieu SQL ni de panne physique revendiqués ; matrice filesystem complète hors de cette preuve.
- [x] Publier simultanément depuis deux clients et montrer toutes les têtes ; réconcilier via T2 quand disponible.
- [x] Tester retrait d'un membre, changement de droits en plein transfert et absence de fuite entre Spaces partageant les mêmes octets.

## Validation et critères de fin

- [x] Contrats/goldens V3 passent et erreurs fermées conservent les codes documentés.
- [x] Tests PostgreSQL réels : aucune admission partielle, événements exacts, races View/politique, clés rejouées, connexions concurrentes.
- [x] Objets corrompus, tailles fausses, capacités expirées, identifiant connu mais non autorisé et CAS manquant sont refusés sans divulgation.
- [x] Le résultat après reprises est identique au parcours sans panne ; aucun pending Step n'est acquitté à tort.
- [x] Un gros transfert n'impose ni chargement complet en mémoire ni verrou exclusif local pendant le réseau. **Qualifié :** publication, clone, capture incrémentale, réception et restauration exactes100Mio/1Gio sur HTTPS réel ; spool privé et projection/admission/matérialisation par chunks, aucun writer Store/transaction SQL local pendant HTTP, Capture concurrente testée. PicOS du processus serveur+clients1Gio :62513152octets, PostgreSQL/cacheOS exclus. [Mesure et limites](measurements/T4-streaming-1gib-20260920.md). Les budgets logiques1Gio/fichier,4Gio de vérification et256Mio de métadonnées restent explicites ; les anciennes API Vec et la fusion sémantique gardent leurs plafonds. Le verrou de journal distant sérialise toujours la reprise exacte de publication.
- [ ] Les scénarios T4.P passent avec deux clients réels ; aucun mock ne remplace le serveur dans la preuve finale. **Acquis :** vraie TLS/PostgreSQL/identités/deux Stores, recette intégrée603,59s ; redémarrages serveur/client Identity et transferts100Mio/1Gio qualifiés séparément ci-dessus. **Manque :** parcours navigateur Web complet et recette combinée de toutes les surfaces ; cette case agrégée reste ouverte.
- [x] `UnavailableV3Facade` n'est plus le chemin de démarrage normal ; les anciennes routes ne sont plus utilisées par les produits.

## Dépendances et parallélisme

| Phase | Prérequis bloquant | Parallèle possible |
|---|---|---|
| T4.C | Accord d'identité avec T8.C | T1.C, T2.C, T3.C |
| T4.E | T4.C | T1.S, T2.M, T8.I |
| T4.L | T4.C + T1.S ; serveur réel pour intégration | T1.R, T3.H, T8.H |
| T4.P | T4.E + T4.L + T8.I + T8.H | Intégrations T5/T6/T7 |

L'engine peut avancer avec une identité de test avant T8.I, mais ne peut être livré ainsi. T8.H dépend de T4.E, pas de T4.P : aucune boucle de dépendance. T4 est propriétaire des schémas/projections métier serveur ; T8 fournit ses migrations d'identité via la même file d'intégration.

Charge indicative : 8–13 semaines-personnes, raccordement serveur/client inclus. Risques : atomisation des reçus, autorisation en concurrence et reprise client. Voir [l'ordonnancement global](01-restauration-et-annulation.md#ordonnancement-global-des-huit-fichiers).
