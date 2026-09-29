# Diagnostic et métriques sans infrastructure supplémentaire

Le serveur écrit un objet JSON par requête sur stderr. Le champ `requestId` est un UUID généré par le serveur et retourné dans `x-request-id`. Un identifiant fourni par le client est ignoré. Conserver cet en-tête lorsqu'une erreur est signalée permet de retrouver la requête dans les journaux du conteneur.

Les champs sont `event=http_request`, `unixMs`, `requestId`, `method`, `route`, `status`, `headersLatencyMicroseconds` et `level`. `route` est le modèle enregistré, par exemple `/v3/spaces/:space/invitations`, ou `unmatched` pour une route inconnue. Les verbes HTTP non standards deviennent `OTHER`. Aucun corps, header entrant, chemin réel, query string, token, mot de passe, identifiant de compte ou message d'erreur arbitraire n'est journalisé. Une erreur HTTP 5xx a le niveau `error` ; un refus 4xx a le niveau `warning`.

## Compteurs HTTP

`GET /v3/admin/metrics` demande un Bearer d'administrateur actif. Les membres ordinaires et requêtes anonymes sont refusés ; la vérification garde le Principal verrouillé jusqu'à la lecture. La réponse porte `cache-control: no-store`.

Dans Desktop/Web : Administration → Activité du serveur → Lire les métriques. Ce bouton utilise la session existante, n'exige aucun jeton saisi et affiche les compteurs, classes HTTP, latence et sondes. Une erreur efface le précédent résultat ; changer de session/projet réinitialise le panneau. Aucun historique n'est persisté dans le navigateur.

CLI : `layrs --json remote metrics --endpoint https://layrs.exemple` après `remote login`. Ajouter `--ca CHEMIN_CA_PEM` pour une autorité privée. La commande fonctionne sans projet local et utilise la session du coffre système ; ne fournir aucun token dans les arguments. Sans `--json`, la sortie reste un document lisible avec les unités nommées.

Le document `layrs.telemetry.v1` fournit :

- Date d'observation et uptime du processus.
- Requêtes en cours, terminées et interrompues avant les headers ; nombres de réponses 1xx à 5xx.
- Somme des latences jusqu'aux headers, en microsecondes, et buckets cumulatifs ≤ 5, 25, 100, 500, 2000 et 10000 ms. Le compteur `completed` sert de bucket +Inf.
- Nombre de sondes readiness et nombres d'échecs PostgreSQL/CAS. Les échecs produisent aussi `dependency_probe_failed` avec deux booléens, sans détail sensible.

Ces compteurs restent en mémoire, de taille fixe et repartent de zéro à chaque redémarrage. Une lecture est une observation concurrente approximative, pas un snapshot transactionnel des compteurs. La requête de métriques elle-même est comptée après sa réponse. La latence ne mesure pas la fin d'un téléchargement streamé ; le compteur d'interruptions ne compte pas les abandons survenus après les headers. Il ne faut pas interpréter les échecs de sondes comme un décompte de toutes les erreurs internes CAS/SQL. Les 5xx HTTP couvrent les erreurs applicatives exposées, sans classification supposée de leur cause.

## Stockage, disponibilité et sauvegardes

`/healthz` vérifie la vie du processus. `/readyz` vérifie PostgreSQL et une écriture/synchronisation/suppression dans le staging CAS. Le healthcheck Compose déclenche ces sondes. [STORAGE-BUDGET.md](STORAGE-BUDGET.md) décrit les mesures de capacité, quotas, seuils et la commande `storage-check` avec codes de sortie exploitables par un ordonnanceur.

[BACKUPS.md](BACKUPS.md) décrit l'ordonnanceur de sauvegarde. Chaque tentative exécutée publie atomiquement `last-backup.json` et `backup.prom`, conserve `lastSuccessUnix` en cas d'échec et retourne un code non nul. Surveiller à la fois l'échec de la dernière tentative et l'ancienneté de la dernière réussite. Un timer jamais installé/exécuté ne produit aucune métrique : l'absence de fichier doit aussi déclencher une alerte.

Ces données peuvent être collectées par les outils existants de l'entreprise. Aucun service de collecte, stockage historique ou alerte externe n'est installé automatiquement. Réserver les logs et fichiers de supervision aux opérateurs autorisés ; appliquer leur politique de rétention.
