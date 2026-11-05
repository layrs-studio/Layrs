# FetchPlan : pages d'un snapshot immuable

La première requête résout la liste complète de claims sous une même identité et
epoch d'autorisation, puis persiste le snapshot complet avant de retourner une
page. Une fois finalisé, le plan et ses capacités restent immuables. Une page ne
modifie aucune référence de dépôt ni aucun checkout.

Le client conserve les mêmes `spaceId`, `clientId`, `requestId` et `claims`, puis
transmet exactement `nextPageCursor` dans `pageCursor`. Le curseur signé lie le
plan, l'acteur, les claims ordonnés, la prochaine position, l'epoch et l'expiration.
Une page traite des claims entiers, jamais une fraction de FileManifest/chunks.
Le budget `maxObjects` vaut 1 à 10 000 ; un claim dépassant ce budget est refusé.
Le snapshot complet est lui-même limité à 10 000 objets/décisions : demander de
petites pages ne permet pas de dépasser cette limite globale.

Une continuation reconstruit et vérifie exactement le snapshot enregistré. Les
résolutions réutilisent un cache borné pendant chaque requête ; une décision ne
peut pas changer entre le calcul de la page et celui du snapshot. Un changement
de sélection de manifeste ou de disponibilité ne réécrit pas l'ancien plan : il
provoque un refus. Une continuation ou une première page rejouée produit les mêmes
capacités tant que l'autorisation et l'expiration restent valides. Les curseurs
invalides sont contrôlés avant toute résolution de graphe.

Après changement d'epoch ou expiration, l'ancien plan/curseur est refusé. Une
nouvelle acquisition exige une nouvelle requête, et les consommateurs doivent
revalider leur état de départ ; aucun renouvellement transparent en cours de
transfert n'est promis. Cette correction n'ajoute aucune migration ni exception
aux triggers d'immutabilité.

Le client natif actuel fige le bootstrap et traverse les identités immuables du
DAG/Tree en demandant un claim par plan. Ce claim est indivisible ; une réponse
`hasMore` dans ce parcours est donc incohérente et reste refusée. Il vérifie toute
la fermeture avant installation/Operation locale. Le batching de plusieurs
claims n'est pas nécessaire à sa complétude et n'est pas ajouté ici.

Qualification : recette HTTPS/PostgreSQL `fetch_pagination_drill.rs`, deux pages,
octets et StepIds exacts, rejeu, mauvais claim/requestId refusés, vrai changement
de droits invalidant curseur et capacités. Tests `v3_plans` : pagination, borne
totale, réutilisation des résolutions et expiration/epoch/signature.
