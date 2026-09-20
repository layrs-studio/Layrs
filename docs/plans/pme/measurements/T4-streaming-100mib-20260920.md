# HTTPS100Mio — qualification complète du20septembre2026

Résultat : **PASS**, 143,376s pour les phases mesurées, 152,053s pour le processus
(installation/authentification/cleanup inclus). Cette mesure conserve séparément
l'échec initial `native-streaming-100mib-20260920-180116.jsonl` : le contrôle dirty
utilisait encore l'API entière bornée64Mio, corrigé par commit `4d3a0a6`.

Commande avec URL PostgreSQL privée déjà définie dans l'environnement :

```text
node scripts/test-native-streaming-volume.mjs --binary D:/Layrs/layrs/tmp/native-streaming-20260920-204610.exe --report D:/Layrs/layrs/tmp/native-streaming-100mib-20260920-204610.jsonl --sizes 100 --restore
```

SHA256 du binaire : `0f847e0db3b1a7530c596e6671cf5737c809650283b3fdaf9fb9545ff9601c88`.
Rapport JSONL et log conservés sous `tmp/`, création exclusive, checkpoints
synchronisés. Aucun token, mot de passe ou URL PostgreSQL inscrit dans ce rapport.

| Phase | Temps |
|---|---:|
| Génération unique100Mio | 0,326s |
| Initialisation/capture | 5,675s |
| Provision | 0,180s |
| Publication | 34,426s |
| Clone | 24,340s |
| Vérification exacte clone + FileId | 0,364s |
| Réception inchangée | 3,864s |
| Capture après modification d'un bloc | 14,528s |
| Publication incrémentale | 14,560s |
| Réception modifiée | 31,596s |
| Vérification exacte réception + FileId | 0,345s |
| Restauration de la Step initiale | 11,877s |
| Vérification exacte restauration + FileId | 0,278s |

Fichier :104857600octets,100blocs uniques de1Mio. Capture en chunks1Mio ; aucune
déduplication artificielle par motif de quelques octets. Une modification touche
exactement le bloc central. Avant/après, empreinte brute et FileId séparés :

| Version | BLAKE3 brut | FileId |
|---|---|---|
| Initiale/restaurée | `c4532dc74e2910457f851ad521a831c88a01e62cab725b335f80945f6282b742` | `blake3:53a06adec6de0c944d18e2ebbb0d2e6aa0596ad3a5c64c64612fc95a91d76e4a` |
| Modifiée | `2a4977cb09b3a8bddc45e8c83d5a2eeff7fae61b0029b6f8db19812f77cc227c` | `blake3:5bfb3130a899323974806fda7dd87871872b6b1115d3ed8eb2a5916ec5624377` |

Mémoire du processus de test : maximumRSS échantillonné57139200octets
(54,49Mio), maximum privé échantillonné18866176octets (17,99Mio), picRSS OS
cumulé57380864octets (54,72Mio). Échantillons50ms : les pics plus courts peuvent
échapper à l'échantillonnage. ServeurHTTPS et clients natifs partagent ce processus ;
PostgreSQL, cacheOS et autres qualifications parallèles sont exclus des compteurs.
Le picOS est cumulatif et ne mesure pas chaque phase séparément. Windows/debug,
une exécution locale avec cache chaud et autres tâches ; aucune promesse de débit
production. Le test1Gio possède son propre rapport et n'est pas qualifié ici.
