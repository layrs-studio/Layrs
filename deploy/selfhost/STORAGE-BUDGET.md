# Capacité et alertes de stockage

Le quota CAS est **logique** : objets catalogués et réservations persistantes,
dédupliqués par ObjectId. Il ne borne pas les fichiers temporaires, les orphelins,
les index/WAL PostgreSQL ni les sauvegardes. Les mesures ci-dessous ne créent
aucune limite physique et ne remplacent pas la supervision du système.

## Commandes d'exploitation

Avec la configuration et les credentials PostgreSQL locaux de l'installation :

```text
layrs-server storage-budget
layrs-server storage-budget space:mon-projet
layrs-server storage-check
layrs-server storage-check space:mon-projet
```

`storage-budget` affiche un rapport JSON. `storage-check` affiche le même rapport
et termine avec un code utilisable dans cron, systemd ou un ordonnanceur Windows :

| Code | Signification |
| --- | --- |
| 0 | Aucun seuil configuré atteint |
| 1 | Avertissement |
| 2 | Critique |
| 3 | Lecture/configuration indisponible ; état inconnu, jamais assimilé à sain |

La commande est ponctuelle : l'opérateur configure sa périodicité et le routage
des notifications. Aucun courriel/webhook ni service planifié n'est installé
implicitement. Conserver les credentials hors des arguments et les rapports
dans un emplacement réservé à l'exploitation.

## Seuils

| Variable | Défaut | Mesure |
| --- | --- | --- |
| `LAYRS_STORAGE_QUOTA_WARN_PERCENT` | 90 | Pourcentage des plafonds CAS configurés |
| `LAYRS_STORAGE_QUOTA_CRITICAL_PERCENT` | 100 | Pourcentage des plafonds CAS configurés |
| `LAYRS_STORAGE_VOLUME_WARN_PERCENT` | 90 | Part du volume CAS indisponible pour le compte serveur |
| `LAYRS_STORAGE_VOLUME_CRITICAL_PERCENT` | 95 | Même volume, incluant les fichiers des autres applications |
| `LAYRS_STORAGE_DATABASE_WARN_BYTES` | absent | Taille PostgreSQL observée, en octets |
| `LAYRS_STORAGE_DATABASE_CRITICAL_BYTES` | absent | Taille PostgreSQL observée, en octets |

Les pourcentages sont entiers, entre 1 et 100 ; l'avertissement doit précéder
le critique. Les seuils PostgreSQL sont des entiers strictement positifs et,
s'ils sont tous deux présents, l'avertissement doit précéder le critique.
Un seuil est atteint à égalité. Aucun arrondi flottant n'est utilisé.

Les plafonds servant de référence aux alertes CAS restent
`LAYRS_CAS_MAX_{TOTAL,SPACE}_{BYTES,OBJECTS}`. Sans plafond, la lecture concernée
porte `monitored: false` ; elle ne représente pas une capacité illimitée garantie.
Sans `spaceId`, seuls le budget global, PostgreSQL et le volume sont évalués.
Planifier aussi une commande par Space si ses plafonds doivent être surveillés.

## API pour l'interface administrateur

`GET /v3/admin/storage-budget` accepte uniquement le Bearer d'un
`bootstrap_admin` actif du déploiement configuré. `?spaceId=space:...` ajoute
le budget d'un Space appartenant à ce déploiement. Le contrôle natif réutilise
expiration, révocation, génération d'autorisation et contrôle Origin. Aucune
authentification par cookie. Réponses `Cache-Control: no-store`.

- Succès : HTTP 200, `schemaVersion: "layrs.storage-budget.v1"`.
- Anonyme, token invalide/révoqué, compte ordinaire ou Origin incorrect : HTTP 401,
  corps `{"error":{"code":"unauthorized"}}`, sans aucun compteur.
- Scope inconnu/hors déploiement pour un administrateur : HTTP 400.
- Observation DB/OS impossible : HTTP 503, sans rapport partiel déclaré sain.

Champs du rapport :

| Champ | Contenu |
| --- | --- |
| `globalUsage` | `objects`, `chargedBytes`, `cataloguedBytes`, `reservedOnlyBytes` |
| `spaceId`, `spaceUsage` | Scope demandé et compteurs, ou `null` |
| `limits` | Plafonds CAS, `null` quand désactivés |
| `thresholds` | Seuils effectifs de supervision |
| `databaseBytes` | `pg_database_size(current_database())`, observation physique distincte |
| `casVolume` | `totalBytes`, `availableBytes`, `freeBytes` renvoyés par l'OS |
| `physicalCasBytes` | `null` : la capacité du volume ne mesure pas l'occupation du seul CAS |
| `readings` | Ressource, unité, valeur `used`, seuils `warningAt`/`criticalAt`, `monitored`, `severity` |
| `severity` | Maximum des lectures : `ok`, `warning`, `critical` |

La capacité vient de `fs2::statvfs` (API OS Windows/Unix), au chemin CAS configuré.
`availableBytes` tient compte de ce que le compte serveur peut utiliser ; il peut
différer de `freeBytes`. Aucun chemin local, secret ou identifiant de connexion
PostgreSQL n'est renvoyé. Les observations DB/OS sont prises successivement,
pas dans un snapshot atomique du disque. Le budget logique conserve les réservations
expirées tant qu'un GC vérifié n'a pas été livré.

## Recette explicite

Sur PostgreSQL jetable avec droit CREATE DATABASE/ROLE, fournir
`LAYRS_TEST_DATABASE_URL` puis :

```text
cargo test -p layrs-server --test native_storage_budget -- --ignored --nocapture
```

La recette crée sa base et son rôle runtime, teste les sessions réelles,
les refus sans fuite, la lecture OS et les codes de sortie du binaire, puis
supprime uniquement ses ressources temporaires.
