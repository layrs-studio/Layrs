# Changement de branche et worktrees par agent — analyse de performance (2026-11-03)

Analyse en lecture seule du code de `feature/worktrees-partial-vfs` (état r208),
faite par trois revues indépendantes puis vérifiée ponctuellement dans le code.
Objectif produit : **changer de branche (Layer) très vite** et **avoir plusieurs
branches simultanées, un Worktree par agent**. Aucune mesure nouvelle ici ; les
chiffres cités proviennent des artefacts existants.

## Verdict

Aujourd'hui, le coût d'un changement de branche est proportionnel à la **taille
du dépôt N**, pas au nombre K de fichiers qui changent. Un switch re-hache tout
l'arbre de travail **6 à 9 fois** et re-vérifie tout le contenu CAS des Steps
concernées **3 fois**. Les agents parallèles sont **sérialisés** par un verrou
Store exclusif et un CAS global par Space. Le VFS, seul chemin conçu pour être
O(K), **refuse** le changement de Layer. Les worktrees partiels (projection)
n'ont pas de switch local.

Mesures existantes (anciennes, à refaire) : switch Layer 6,3 s à 10k fichiers et
58,6 s à 99k (`docs/plans/pme/measurements/T8-local-volumes.md`, 2026-09-20,
petits fichiers, cache chaud) ; création de worktree 21–25 s pour 32 fichiers
(debug, R7) ; status 17–18 s à 1k fichiers (release baseline), 95–128 s avec le
candidat R7. Le protocole L0 actuel ne mesure **pas** le switch.

## 1. Chemins existants

| Chemin | Switch de branche | Création d'un worktree | Coût dominant |
| --- | --- | --- | --- |
| Store complet (`layrs layer use`, Desktop `switchLayer`) | Oui : `NativeWorkspace::switch_layer` → `apply_projection_transition` → `scm_v1::materialize` | `layrs worktree create` : copie physique O(N) | Re-hash complet de l'arbre, re-vérification CAS, fsync par fichier |
| Projection format-4 (famille partagée) | Non en local ; seulement `remote receive-partial` (réseau, `refresh_projection`) | Membre de famille : métadonnées seules, mais pas de CLI « add member » | Re-hash des fichiers hydratés, validations admises répétées |
| VFS Windows (CFAPI) | **Refusé** (`vfs_worktree.rs:960`) | Placeholders sans hydratation (bon modèle), mais tous créés d'avance | Peuplement O(N) placeholders, hydratation fichier entier |

### Switch sur Store complet — étapes et passes

1. Capture avant switch (`native_scm/restore.rs:157`) : scan complet, 2–3 passes.
2. Lecture des projections cible et base : `file_availability_with_budget` relit
   et **re-hache chaque chunk** de chaque fichier (`scm_v1/read/availability.rs:171-213`), O(octets du dépôt), ×3.
3. Matérialisation (`scm_v1/materialize.rs`) : pré-contrôle par capture complète
   (`:250`), staging des K fichiers (copie + fsync + 2 re-hash chacun, réouverture
   du Store **par fichier** `restore.rs:447`), journal en 3 phases avec ~5 fsync
   par fichier, post-contrôle par capture complète (`:556`).
4. Desktop relance ensuite un `working_diff` (2–3 passes de plus).

`capture_streaming` fait au moins deux passes identiques (`checkout/streaming.rs:17-30`).
Aucun index stat/mtime/USN n'existe ; chaque `connection()` re-vérifie la chaîne
d'identité des répertoires (`scm_v1/layout.rs:270-303`) — 86,5 % des E/S
supplémentaires du diagnostic L6 `TlGfms`.

### Agents parallèles — sérialisation

- Toute écriture prend le `layrs.lock` du Store **exclusif**, y compris pendant
  l'installation CAS et ses fsync (`scm_v1/repository.rs:257-325`) ; les lecteurs
  et l'hydratation VFS des autres worktrees ne font que sonder (try-lock, 5 s).
- CAS optimiste **global par Space** sur `current_repository_views` /
  `operation_heads` (`repository.rs:475-540`) : la RepositoryView embarque l'état
  de tous les worktrees. Une capture dans un autre worktree fait échouer un switch
  (pas de retry) ou une création (testée comme « doit perdre »).
- Famille : `catalog.lock` exclusif sans retry pour chaque réservation/publication,
  journal SQLite `DELETE` + `synchronous=FULL`, quota global unique (SUM complet
  par réservation), `lifecycle.lock` tenu par tout provider (GC impossible).
- Mesures : deux agents sur un Store, deux `step` simultanés 8,3 vs 14,6 s (le
  second attend) ; cellule partagée `z6rF0T` en échec `LockBusy`.

### Coût par opération sur worktrees admis (C14)

Une opération déclenche de nombreuses validations complètes du lecteur admis
(`read_admitted_import`) : Capture ≈ 33 + 6K, Restore ≈ 50 + 10K + 12P. Chaque
validation relit 5 fois le préfixe importé, recrée ~12 fois le schéma de
référence en mémoire et lance 6 `foreign_key_check` complets — sur des données
**immuables**. Les worktrees ordinaires (non admis) hachent tout l'arbre à chaque
scan (3 scans par capture, 4 ouvertures par fichier).

## 2. Goulots classés pour « switch très rapide » et « un worktree par agent »

| # | Goulot | Preuve | Direction |
| --- | --- | --- | --- |
| 1 | Re-hash complet de l'arbre 6–9× par switch, aucun index stat | `restore.rs:157`, `materialize.rs:250,556`, `streaming.rs:17-30` | Index persistant par worktree (taille, LastWriteTime, ChangeTime, FileId, USN) comme **accélérateur** avec repli exact (contrat 10) ; réutiliser une seule capture ; post-vérifier seulement les K chemins touchés |
| 2 | Re-vérification CAS complète à chaque lecture de projection | `read/availability.rs:171-213` | Projection construite depuis les métadonnées (contenu vérifié à l'admission) ; diff structurel des Trees (TreeId égaux sautés) → O(K·profondeur) |
| 3 | Verrou Store exclusif long + CAS global par Space | `repository.rs:257-325,475-540`, `lock.rs:46-52` | Lignes View/Operation par worktree ; installation CAS idempotente hors verrou ; SQLite WAL pour lecteurs ; retry sur CAS mismatch |
| 4 | Matérialisation par fichier lourde (Store rouvert par fichier, ~5 fsync, copie complète) | `restore.rs:447`, `materialize.rs:450-553` | Un lecteur partagé, fsync de répertoires groupés, block-clone ReFS/Dev Drive depuis le CAS quand disponible (jamais de hardlink modifiable) |
| 5 | Vérifications d'identité de chemin à chaque requête SQL / tick | `layout.rs:270-303`, `materialize/lease.rs:51` | Vérifier une fois par ouverture/transaction via handles |
| 6 | Validations admises répétées (C14) | `admitted_private_io.rs`, `read.rs:611-655` | Dédup interne (×3–5), mémo exact par accès (×4–6), puis séparation immuable/mutable |
| 7 | VFS : pas de switch, placeholders tous créés, hydratation fichier entier | `vfs_worktree.rs:960`, `windows/mod.rs:332-397` | Switch par remplacement de placeholders (`CfUpdatePlaceholder`) des K chemins, population à la demande, lecteur provider persistant |

## 3. Cible et ordre proposé

Cible : **switch O(K)** (fichiers réellement différents entre les deux Steps +
fichiers modifiés localement) et **création de worktree en O(métadonnées)**,
agents indépendants sans verrou global sur le chemin chaud.

1. **Mesurer d'abord** (L0) : ajouter `layer use` au protocole de mesure (1k,
   10k, 100k fichiers ; release), plus création de worktree et deux agents
   simultanés. Sans baseline, pas de gain prouvé.
2. **Gains sûrs et locaux** : dédup des validations (C14) ; une seule capture par
   switch avec post-vérification limitée aux K chemins ; projection depuis
   métadonnées ; identité de chemin vérifiée une fois par ouverture.
3. **Index stat/USN par worktree** (accélérateur avec repli exact), réutilisé par
   status, capture, switch et diff.
4. **Concurrence** : état View/Operation par worktree, verrou exclusif réduit au
   commit SQLite, WAL, retry ; famille en WAL avec retry borné et quotas par
   worktree.
5. **Switch VFS** par remplacement de placeholders O(K) et création de worktree
   VFS instantanée ; block-clone pour les worktrees physiques.

Chaque étape : revue indépendante (sécurité des invariants de vérification),
tests existants verts, mesure avant/après sur les mêmes jeux.
