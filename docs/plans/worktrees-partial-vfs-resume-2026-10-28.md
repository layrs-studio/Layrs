# Reprise du 28 octobre 2026

## Point courant - conseil s13 (Q1-Q7), s14 à s16, format 7 (Claude + conseil, 2026-11-12)

Décisions du conseil s13 (brief `council-s13-design.md`) : Q1 packs inline jusqu'à 256 Kio ; Q2 fermeture prouvée par les bits d'admission ; Q3 plus de relecture des dossiers dans les captures qui ne font qu'enregistrer ; Q4 `room merge` fusionne la dernière étape capturée (`--capture`) ; Q5 recette `whole` derrière un nouveau format ; Q6 schéma v4 ; Q7 VFS SQLite durci, avant Q1, Q5 et Q6.

Commits :
- `46ce285` Q2-Q4, `3f06b3e` corrections s14 r1, `0df6c5d` (seuls les manifestes admis *sans* être portés par le lot forcent le chemin exact), `7e30e34` corrections s14 r2. Conseil s14 : r1 Astra 6, Sol 7, Opus 8 ; r2 Astra 9 (ship), Sol 8, Opus 7.
- `718ddd9` Q7 : `guarded_vfs`, un VFS qui n'ouvre que la base (et son `-wal` pour les écrivains du Store), cache les journaux et `-shm`, ne supprime rien d'autre ; cache dérivé et écrivains du Store passent par lui (`locking_mode=EXCLUSIVE` avant la première lecture).
- `0054b8c` Q1 : packs de lot inline jusqu'à 256 Kio, fusion des inline à 8 packs ou 1 Mio, sortie de fusion inline seulement jusqu'à 64 Kio.
- `acd93ed`, `27b798b`, `8e07476` corrections s15 (Astra 7 → 8, Sol 6 → 9 ship, Opus 8 → 8) : `SQLITE_OPEN_NOFOLLOW` sur tous les écrivains, nom refusé *avant* l'ouverture s'il est un lien, identifiant de fichier 128 bits (repli 64 bits), reprise du nom d'une base liée (copie privée, mode et propriétaire de la source).
- `ed92a45` Q5 : format 7 (`LAYRS_STORE_FORMAT=7`, opt-in) = format 6 + recette `whole` (un fichier de 1 octet à 1 Mio a un seul chunk nommé par son FileId) + packs v2 de version 2. `8b5a419` : valeurs du cache dérivé en trames zstd (cache v2).
- Q6 (schéma v4) retiré (`3b07408`) : WITHOUT ROWID cassait le `rowid` de l'historique et de fsck, pour ~8 Ko. Reporté aux identifiants binaires (ADR 0028 §7).
- Corrections s16 : r1 `3b07408`, `0aaa289`, `5df6730` (Astra 5, Sol 5, Opus 5) ; r2 `d279b1f`, `818ebdb`, `fc50f97` (Astra 7, Sol 7, Opus 7). Règle d'une seule forme par fichier (`prepare::redundant_file_objects`, captures, réceptions, réécritures) ; barrière `whole` à chaque `validate_stored_bytes` ; consolidation mixte whole/v1 dans les deux ordres ; le réseau ne porte que du v1 (`refuse_whole`). r3 `affe82d` (Astra 8, Sol 7, Opus 8, tous ship after fixes) : routage des checkouts (`checkout_kind`, Studio) par `scm_v1::supported_format` (le format 7 était refusé) ; plan des enregistrements avec les chunks `whole` externes ; le v1 reconstruit à la réception vient toujours d'une lecture vérifiée du Store courant (seuls les manifestes en cache) ; fusions et résolutions en une seule forme par fichier (`prepare::new_objects`, une recherche d'admission par objet) ; tests des refus des packs v2. Trois tours : arrêt du conseil sur ce lot.

| Membre | s16 r1 | s16 r2 | s16 r3 |
| --- | --- | --- | --- |
| GPT-6 Astra (xhigh) | 5/10 | 7/10 | 8/10 |
| GPT-6.1 Sol (max) | 5/10 | 7/10 | 7/10 |
| Claude Opus 5.5 (xhigh) | 5/10 | 7/10 | 8/10 |

Suites Windows à `fc50f97` : store-local 784 en formats 7, 6 et 5 ; client-core 284 en 7 et 6 ; CLI 113 en 7 et 5 ; core 35 ; vfs 48 : tout passe. À `affe82d` : store-local 786 (7, 6, 5), client-core 285 (7, 6), CLI 113 en 5 et 112/113 en 7 : `physical_room_new_step_merge_drop_and_recreate` a échoué une fois sur `room merge` avec « Accès refusé (os error 5) » (machine chargée), puis 10/10 ; core 35 ; vfs 48. Même famille que `room_merge_carries_untouched_working_changes_and_refuses_touched_ones` : un renommage ou une ouverture refusés par Windows pendant une matérialisation (antivirus ou suppression en attente). git réessaie ces erreurs (`mingw_rename`) ; à traiter.

Audit Fable 5.2 (à la demande de l'utilisateur, « tourner en rond ? ») : **NON**, continuer. Signaux qui feraient passer à OUI : deux bancs précis de suite sans écart réduit de plus de ~5 % ; Q5 repoussé une troisième fois ; un conseil au-delà de 3 tours ; une nouvelle réécriture du cache de projection ou de l'index. Choix produit à confirmer par l'utilisateur : Rooms physiques par défaut (s10 Q5) ; `room merge` ignore les modifications non capturées (s13 Q4). Écart non classé : `init` ~10 s contre 0,5 s pour git quand Defender connaît déjà les contenus.

Banc précis (10 000 fichiers, **format 7**, binaire `8b5a419`, deux passes) : Store 1 180 / 1 636–1 640 Kio (git 1 892 / 1 932) ; `step` 82–83 ms (git 71–73) ; `room step` 87–91 ms ; avance rapide 80–103 ms (git 40–43) ; 2e fusion 84–86 ms (git 43–44) ; 3 voies 223–234 ms (git 253–265) ; `status` 24 / 30–33 ms (git 22 / 21) ; création de Room 1,11–1,19 s (git 2,0) ; suppression 89–91 ms (git 1 140).

Après s16 (`010aa49`, `715b33e`) : `status` ne lit plus un fichier non garanti par l'index dont la longueur diffère de la base, ou que la base n'a pas (git compare les tailles avant de hacher), et plus aucun dès qu'un changement est établi : il ne répond que « modifié ou non » (les réponses dont dépend une écriture lisent comme avant ; test avec des fichiers illisibles). `step` lit sa base dans la session ouverte par la découverte (3 → 2 ouvertures). Banc précis (format 7, binaires 138 et 139, deux passes chacun) : `status` 2 changements 30,4–32,7 → 24,5–26,1 ms (git 21,2–23,7) ; `status` propre 23,7–25,1 (git 21,5–23,3) ; `step` 85–87 (git 74–84) ; avance rapide 83–85 (git 43–46) ; 2e fusion 81–85 (git 42–44) ; 3 voies 221–232 (git 254–269) ; Store 1 228–1 232 / 1 656–1 668 Kio (git 1 916 / 1 956–1 980 ; le retrait de Q6 rend ~48 Kio).

CI de `main` : Linux a d'abord échoué sur deux tests ajoutés en s16 qui supposaient un Store compact (le format par défaut, 5, ne prouve pas la fermeture par bits d'admission) : corrigés (`971e0a8`, `7e5bb5e`). Puis tout le Rust passe sous Linux ; seul le test Playwright `studio-web-contexts-native` a échoué (« Droits du projet mis à jour » absent en 5 s), déjà vu le 5 novembre, sans rapport. Tests Unix de store-local lancés en local dans WSL (278, dont les liens et `-wal`) : verts.

Profil de l'avance rapide (72 ms dans le processus, binaire 133) : matérialisation 46,5 ms dont journal de session 10,8, métadonnées et clôture 7,1, phases 5,0 (barrière NTFS 3,7), index de changements 4,7, purge 2,5 ; 5 ouvertures partagées du Store 8,8 ms ; deux lectures de projection 7,1 ms. L'essentiel de l'écart est le protocole de reprise durable (décision utilisateur n° 1 ci-dessous). Piste sans compromis : une ouverture du Store par commande (5 → 1 : ~5 ms sur `step`, `room step` et l'avance rapide ; `room step` découvre aussi le client trois fois, ~5 ms).

**Phase 6 en cours (conseils s17, s18, 2026-11-12)** : B2a `b915318` (consolidation due seulement si un groupe tient) ; B1a `7212ac4` (écrivains à preuve positive, `with_positive_evidence` ; lot de réparation d'une capture inchangée ; `step --full` rapporte le contenu perdu depuis le dossier) ; B1b `e016b79` (ligne `cas_pack_keys` par pack : ensemble Golomb-Rice maison, 20 bits, ~21,5 bits par clé, `keys_digest` et condensés par 256 ordinaux ; dernière modification du schéma v3 : les Stores 6 et 7 créés avant sont refusés ; +64 Kio à 10 000 fichiers) ; B1c `525f5f7` (fsck recalcule chaque ligne de clés). Conseil s18 (revue) : r1 Astra 6, Sol 6, Opus 6 (`e2bfd23`) ; r2 Astra 7, Sol 6, Opus 5 (`6accdb5` : P0 de Sol et Opus, la capture avant suppression ou réécriture d'un dossier lit tout et exige la réparation à côté d'un pack illisible) ; r3 Astra 6, Sol 7, Opus 6 (`eb37c0a`, `3f065bf` : Room virtuelle non supprimée tant qu'un pack est illisible ; préchargement des manifestes et fsck ; une Step durable reste close par induction, sinon un vieux pack de contenu perdu bloquait tout lot ; remède nommé selon le refus). Trois tours : arrêt du conseil s18. Non faits : récupération par la capture vérifiante d'une Room virtuelle ; repli du cache de projection borné à l'arbre fautif ; test d'une récupération obligatoire refusée. Limite : un pack perdu qui tenait une Step de l'historique bloque les Steps suivantes (ascendance) jusqu'à la réparation exacte (reconstructible depuis `steps` et `step_parents`). B2b fait : `layrs maintenance` (`scm_v1::maintain`) ; un passage à la fois (`metadata/maintenance.lock`) ; groupe géométrique sous la moitié des limites du lecteur seulement ; pack construit hors verrou sous `objects/packs/.maint-<id>.tmp` (synchronisé, vérifié ; un passage planté le laisse, le suivant le retire) ; sous verrou exclusif, chaque candidat doit être encore inscrit tel quel, sinon abandon ; publication sans écrasement et remap exact des admissions (celles faites pendant la construction sont gardées). B2c fait : un lot qui n'a pas consolidé note la maintenance due (au moins 6 fichiers de pack hors packs écartés, et un groupe de maintenance) ; la CLI, sortie écrite, lance `layrs __layrs-maintenance <racine du Store>` détaché (sans fenêtre, groupe de processus propre, poignées nulles, dossier de travail hors checkout) sauf si un passage tient le verrou ; jusqu'à 8 passages ; `LAYRS_AUTO_MAINTENANCE=0` le coupe. Desktop et Studio Web ne le lancent pas encore. Restent : mesures (100 000 fichiers, Store de plus de 64 Mio), passage du format 7 par défaut.

**Conseil s17 (conception, 2026-11-12)** : plan de la phase 6 en huit pas, consigné dans `store-format-6.md` (« Council s17 »). B2a fait (`b915318`). B1a commencé : test `native_lost_pack.rs` gardé hors commit (`scratchpad/lost_test_v3.py`) ; il montre qu'une capture complète d'un dossier égal à sa base doit porter un lot de réparation (contenu et admissions, sans nouvelle étape) quand le Store ne peut plus prouver son contenu.

**Format par défaut** : `init` crée encore du format 5 ; le Store ne bat git qu'en format 7. Le passage est la phase 6 de l'ADR 0028, bloquée par deux points décidés par le conseil et non faits (`store-format-6.md`, « Blockers before format 6 becomes the default ») : un pack inscrit illisible refuse tout le Store (s2 r1 : filtres `cas_pack_keys`, packs `lost`, réparation exacte) ; les tranches au-delà des budgets inline ne sont jamais consolidées (maintenance `store gc`). Prochain chantier pour la taille.

**Reste** : purge des opérations de récupération terminées (clé d'idempotence) ; identifiants binaires dans SQLite ; écarts `status`, `step`, fusions ; test instable `room_merge_carries_untouched_working_changes_and_refuses_touched_ones` (« Accès refusé », Windows, 1/10 avant ce travail).

## Point courant - caches dérivés en SQLite, conseil s12 (Claude + conseil, 2026-11-12)

Constat de mesure (Windows 11, Defender) : un fichier de données fraîchement écrit coûte ~5–9 ms à sa première ouverture par un nouveau processus, quelles que soient sa taille et son extension ; une base SQLite ~0,7 ms. Chaque commande relisait l'index de changements et les projections écrits par la précédente.

Commits (`4bcf9d3`, `acf47f2`, `7888a35`, corrections `cfc2f97`, `caffd37`, `ef14128`) :
- `metadata/derived.sqlite3` range les index de changements et le cache de projections (valeurs inchangées, sommes de contrôle comprises). Contrat dans l'ADR 0025 : journal en mémoire, lecteurs en lecture seule, fichiers annexes refusés ou retirés, fichier à plusieurs liens jamais écrit (handle de SQLite vérifié sous Windows), base abîmée remplacée, base occupée ou plus récente laissée en place.
- `status` ne construit plus les arbres pour valider une projection qu'il n'enregistre pas (contrôles de forme gardés).
- Une capture inchangée ne reconstruit plus les arbres quand un fichier « racy » a été relu ; spool en mémoire jusqu'à 4 Mio.
- Cache de projection format 5 : un fichier tenu par un grand pack (≥ 4 096 enregistrements) y est nommé par (pack, rang) au lieu de son FileId de 32 octets ; quarantaine dans le processus d'une projection trouvée fausse.

| Membre | s12 r1 | s12 r2 | s12 r3 |
| --- | --- | --- | --- |
| GPT-6 Astra (xhigh) | 5/10 | 6/10 | 6/10 |
| GPT-6.1 Sol (max) | 6/10 | 6/10 | 6/10 |
| Claude Opus 5.5 (xhigh) | 7/10 | 6/10 | 8/10 |

Tous « ship after fixes » ; corrections de r3 faites (`ef14128`). Trois tours : arrêt du conseil sur ce lot. Risque résiduel enregistré (ADR 0025), commun avec la base du Store : un processus qui plante un journal ou un WAL entre nos contrôles et le verrou de SQLite. **À faire** : un VFS qui n'ouvre que la base principale, pour les deux bases.

**Banc précis à 10 000 fichiers, format 6** (`bench_py.py` : processus lancés par CreateProcess, sans shell ; deux répétitions ; binaire `ef14128`) :

| | layrs | git |
| --- | --- | --- |
| `status` propre / 2 changements | 24–26 / 32–33 ms | 20 / 19 ms |
| `step` (2 fichiers) | 97–101 ms | 65–67 ms (`add` + `commit`) |
| création de Room | 1,10–1,12 s | 1,93–2,34 s |
| `room step` | 93–99 ms | 65–206 ms |
| fusion en avance rapide / 2e fusion | 100 / 100–103 ms | 38–45 / 40 ms |
| fusion 3 voies | 232–243 ms | 249–253 ms |
| suppression de la Room | 75–76 ms | 1 138–1 163 ms |
| Store après `init` / après le scénario | 1 752 / 2 180–2 184 Kio | 1 844 / 1 884 Kio |

Le banc bash (`bench_vs_git_samezone.sh`) ajoute ~35 ms par processus aux deux outils et masquait ces écarts. `init` dépend de l'antivirus : contenus neufs, `git add` 35 s contre `layrs init` 10 s ; contenus déjà vus par Defender, git 0,5 s, layrs toujours ~10 s (binaire non signé, fichiers réanalysés à chaque ouverture).

**Écarts restants** :
- fusions (~60 ms) : capture de la Room avant fusion (décision actée) et matérialisation journalisée (Q1) ; dans les règles actuelles, plancher estimé ~40 ms dans le processus (ouvertures du Store, relectures). Rouvrir ces décisions revient à l'utilisateur.
- `step` (~30 ms) : validation de fermeture du lot (~10 ms pour ~2 000 fichiers des dossiers modifiés), pack écrit en fichier au-delà de 64 Kio avec sa barrière.
- `status` (~5–14 ms) : découverte et ouverture du Store (~7 ms), deux ouvertures de la base dérivée.
- Store après le scénario (+300 Kio) : Q3 (~330 Kio, changement d'identité) et schéma SQLite du format 6 (457 Ko après le scénario : 44 Ko de texte de schéma, 93 arbres d'une page, 40 % d'espace inutilisé).

## Point courant - décisions du conseil s10, revue s11 (Claude + conseil, 2026-11-11)

Les décisions laissées à l'utilisateur sont restées sans réponse ; l'utilisateur ayant délégué (« hésite pas a faire appel au conseil pour vérifier ou décider »), le conseil s10 les a tranchées (brief `council-s10-design.md`) :
- Q1 (unanime) : garder journal et sauvegardes ; une petite matérialisation (≤ 64 chemins, ≤ 16 Mio, fichiers ordinaires, sans bascule fichier/dossier, un volume NTFS) tient une seule session exclusive du journal à la clôture, avec une seule validation. Fait (`d82e38d`) : `room.fast_forward` 97–101 → 64 ms dans le processus.
- Q2 (unanime) : `room merge` ne capture plus le dossier du projet (sémantique de git) ; les chemins écrits restent prouvés juste avant remplacement. Fait (`9b3b2de`). `layrs weave` garde sa capture.
- Q3 (unanime) : un seul condensé de 256 bits pour un fichier à un morceau, par un changement d'identité versionné. **Non commencé** (gain estimé ~330 Kio à 10 000 fichiers ; seul, il ne rejoint pas git).
- Q4 (unanime) : `room drop` d'une Room physique se fie à l'index de changements (deux passes, entrées racy relues) ; `--full` relit tout. Fait (`9b3b2de`).
- Q5 (Astra, Sol ; Opus convergent pour un projet de petits fichiers) : Rooms physiques par défaut, `--virtual` explicite. Fait (`9b3b2de`). Les mentions « virtuelles par défaut » et « drop relit tout » plus bas sont dépassées.

Autres commits : `06aeb8b` (format 6 en `auto_vacuum = FULL` : Store après le scénario 2 620 → 2 556 Kio) ; `294d6c0` puis `5f144d7`, `8ef78d0`, `84dba82` (une création à neuf tient chaque fichier écrit jusqu'à la fin de la marge de dates avant de dater sa preuve : première `room step` 1 064 → 137 ms ; sans cela, preuves datées de chaque écriture, donc racy) ; `f506652` et la correction r2 (Store relâché dès la clôture, écrivain fermé avant le verrou, refus avant écriture d'un dossier remplacé contenant des non-suivis, fusion en conflit refusée sur dossier modifié, tests d'arrêt brutal d'une session tenue).

| Membre | s11 r1 | s11 r2 | s11 r3 |
| --- | --- | --- | --- |
| GPT-6 Astra (xhigh) | 6/10 | 6/10 | 8/10 |
| GPT-6.1 Sol (max) | 8/10 | 8/10 | 8/10 |
| Claude Opus 5.5 (xhigh) | 7/10 | 8/10 | 8/10 |

Corrections r3 (`3f94709` et suivant) : passes de la capture avant suppression décrites telles qu'elles sont (celles de la vérification complète) ; assertion du WAL du test d'arrêt brutal réservée à Windows (seul cas de session tenue) ; un dossier vide non suivi sous un dossier remplacé par un fichier est refusé avant le journal ; `projectDirty` vaut `null` hors `--preview`. Trois tours faits : arrêt du conseil sur ce lot.

**Banc à 10 000 fichiers, format 6** (`c5d82a4`, deux répétitions ; git dans la même zone) :

| | layrs | git |
| --- | --- | --- |
| `init` | 1,2–1,6 s | 41,7–42,4 s |
| `status` propre (juste après `init`) / 2 changements | 57–59 / 61–69 ms | 46–48 / 51 ms |
| `step` (2 fichiers) | 123–137 ms | 141–145 ms |
| création de Room (physique par défaut) | 1,22–1,31 s | 2,11–2,18 s |
| première lecture de tous les fichiers | 2,53–2,61 s | 2,54–2,68 s |
| `room step` | 127–132 ms | 231–259 ms |
| fusion en avance rapide / 2e fusion | 136–145 / 153–173 ms | 68–69 / 66–68 ms |
| fusion 3 voies | 270–307 ms | 303–357 ms |
| suppression de la Room | 0,11–0,12 s | 1,31–1,34 s |
| Store après le scénario | 2 556–2 564 Kio | 1 822 Kio |

`status` juste après `init` : ~1 000 fichiers écrits moins de 100 ms avant la preuve de l'`init` sont relus (la preuve d'une capture est datée d'avant l'énumération ; git date son index après le hachage, compromis plus faible). En régime établi, `status` est devant git.

**Inachevé** : Q2 n'est appliquée qu'à `room merge` ; `layrs weave` capture toujours le dossier avant de préparer (enregistré dans l'ADR 0025).

**Écarts restants contre git** (banc `9b3b2de`) : fusion en avance rapide ~150 ms contre ~78 (la capture d'une Room physique coûte ~33 ms sans sentinelle ; la capture de la Room avant fusion est une décision actée) ; Store 2 556 contre 1 822 Kio (Q3 ~330 Kio ; ~420 Kio de structure SQLite sans équivalent chez git).

## Point courant - lectures de packs, captures, fusions (Claude, 2026-11-09, suite)

**Commits**
- `dfdecba` : purge du dossier de tentative sans synchronisation (rétablie par `a138aa1`) ; parents de la phase 2 sans synchronisation propre sur un volume NTFS (la barrière les rend durables, décision B2 2 du conseil s2) ; la création du dossier de tentative reste synchronisée (même décision) ; un delta de projection lié à la dernière projection complète lue par le processus (même condensé) s'applique à ses octets vérifiés sans relire le fichier ; `status` décode l'index de changements sur un autre fil pendant la lecture du Store.
- `21c22a5` : la capture ne crée son fichier de spool qu'au premier morceau ; une capture qui n'a rien relu et a fait confiance à toutes les entrées garde l'index tel quel (au lieu de le reconstruire entrée par entrée) ; la projection du nouvel arbre est écrite dans le cache pendant l'acceptation.
- `bedeb35` : la capture décode aussi l'index pendant la lecture du Store.
- `6ffc7d7` : un catalogue garde ouvert chaque fichier pack qu'il lit (avant : une ouverture épinglée par bloc lu) ; cache de blocs 8 → 64 ; limité aux lectures en masse par `a138aa1`. Lecture des morceaux à la création d'une Room virtuelle : 2 322 → 47 ms à 10 000 fichiers.
- `89cede5` : recontrôle avant fusion en une passe — annulé (conseil s9 r1, voir plus bas).
- `b7dcca4`, `625c6b1` : un seul verrou partagé du Store par première lecture d'un fichier virtuel (manifeste et morceaux) ; une lecture annulée pendant l'attente du verrou reste `Cancelled`.

**Banc à 10 000 fichiers, format 6** (`625c6b1`, avant les corrections du conseil s9, qui limitent les handles de packs gardés aux lectures en masse ; deux répétitions, machine calme ; git dans la même zone) :

| | layrs | git |
| --- | --- | --- |
| `init` | 1,2–1,7 s | 40–48 s |
| `status` propre / 2 changements (juste après `init`) | 56–57 / 63–65 ms | 51–55 / 53–60 ms |
| `step` (2 fichiers) | 126–136 ms | 150–155 ms |
| création de Room (agent) | 1,68 s | 2,17–2,22 s |
| première lecture de tous les fichiers | 3,37–3,55 s | 2,67–2,96 s |
| `room step` de l'agent | 164–210 ms | 175–238 ms |
| fusion en avance rapide / 2e fusion | 143–167 / 147–148 ms | 72–79 / 69–74 ms |
| fusion 3 voies | 319–337 ms | 359–371 ms |
| suppression de la Room | 0,29–0,45 s | 1,41–1,47 s |
| création d'une Room physique | 1,29–1,40 s | — |
| Store après le scénario | 2 624–2 628 Kio | 1 822 Kio |

Après les corrections du conseil s9 (`9fd414a`, deux répétitions, machine plus chargée : `git init` 54–58 s) : `status` 61–64 / 64–66 ms (git 55–62 / 59–64) ; `step` 134–136 (151–179) ; création de Room 1,83–1,95 s (2,26–2,57) ; première lecture 3,46–3,93 s (2,91–3,30) ; `room step` 204–242 ms (168–188), mais 156–177 ms sur quatre séries isolées de cinq essais, autant qu'avant les corrections (`room_step_cmp.sh`) ; fusion en avance rapide / 2e 162–172 / 147–161 (78–83 / 80–83) ; 3 voies 335–361 (385–416) ; suppression 0,35–0,53 s (1,38–1,52) ; Room physique 1,45 s ; Store 2 628–2 636 Kio (1 822–1 826).

Notes :
- `status` juste après `init` : les fichiers écrits moins de 100 ms avant la preuve de l'`init` sont « racy » (marge NTFS) et relus une fois, comme git relit ses entrées racy ; en régime établi, `status` est devant git (21,1–22,0 contre 22,5–22,8 ms).
- Première lecture d'une Room virtuelle : même une deuxième lecture coûte ~0,4 s de plus que git (2,97 contre 2,54 s). L'hôte ne reçoit aucun rappel pendant ces lectures : c'est le filtre ProjFS à chaque ouverture d'un placeholder hydraté. Création et lecture ensemble restent à parité (5,05–5,23 contre 4,84–5,18 s) ; une Room physique se lit comme un dossier ordinaire.

**Mesures ponctuelles**
- `status` propre, même dossier : 21,1–22,0 ms contre 22,5–22,8 ms pour git (avant : 22,9 ms).
- `step` sans changement : 29 → 24 ms dans le processus.
- Création de Room : virtuelle 2,08–2,29 → 1,73–1,75 s, physique 1,43–1,47 → 1,18–1,23 s (git `worktree add` 2,29 s, mesures séparées de `room_create_bench.sh`).
- Fusion en avance rapide, dans le processus : ~160 → ~115 ms (git ~45 ms hors démarrage).

**Conseil s9 (revue de `5435358..9ca9ddf`, brief `council-s9-r1.md`)**

| Membre | s9 r1 | s9 r2 | s9 r3 |
| --- | --- | --- | --- |
| GPT-6 Astra (xhigh) | 8/10 | 8/10 | 8/10 |
| GPT-6.1 Sol (max) | 8/10 | 8/10 | 8/10 |
| Claude Opus 5.5 (xhigh) | 8/10 | 8/10 | 9/10 (`ship`) |

Corrections s9 r3 (dernier tour) : un fichier de projection retiré est supprimé avant d'être oublié du mémo (Astra, Sol, Opus) ; mémo expiré libéré ; au plus 32 packs tenus pour tout le processus (Astra) ; la vue tenue pendant un peuplement est celle en cache au moment de la prise (ordre des verrous de `bulk_reads`, Opus) ; test `a_bulk_read_holds_only_the_current_view` (Opus) ; ADR 0026 (bornes, cache de blocs 8 → 64).

Corrections s9 r2 (`7138c99`) : un peuplement de Room ne tient que la vue CAS courante (les trois : une prise s'ajoutait à chaque nouvelle vue, toutes les 2 s) ; le mémo de projection n'est republié par aucune lecture commencée avant un oubli (génération) et ne sert qu'une seconde (Astra) ; test `pack_files_stay_open_only_while_held` (Opus) ; ADR 0026 précisée (au plus 16 packs par catalogue tenu, une vue de l'hôte, une session par fil de staging).

Corrections s9 r1 (`64234fa`, `a138aa1`) :
- Handles de packs (les trois) : gardés seulement pendant une lecture en masse explicite (`hold_pack_files` : peuplement d'une Room par le crochet `SnapshotSource::bulk_reads`, staging d'une matérialisation), au plus 16, en lecture seule sans droit de suppression ; ADR 0026 amendée. Hors de ces fenêtres, une ouverture par lecture comme avant.
- Purge du dossier de tentative à nouveau synchronisée (Sol, Opus) : les journaux complétés nomment toujours ce dossier, qui ne serait jamais balayé.
- `89cede5` annulé (Opus) : une première passe égale à la base clôt déjà la vérification en deux passes ; le changement ne gagnait rien sur un dossier propre et affaiblissait D4 ailleurs.
- Mémo de projection borné (16 Mio) et oublié quand la projection est retirée ou illisible (Astra, Opus) ; test `a_removed_base_no_longer_answers_its_deltas`.
- `components_to_parent_with(synced)` au lieu de deux copies (Opus).

**Écarté pour l'instant** : une seule session exclusive tenue du journal à la clôture (décision unanime s4 Q3). Tenir le verrou exclusif pendant les phases rend caduc le modèle de baux (un concurrent ne peut plus reprendre un bail en cours de phases, ce que testent `native_materialize_lease`) ; gain estimé ~8 ms par petite matérialisation. À reprendre avec une revue du conseil.

**Décisions utilisateur en attente**
1. Fusion en avance rapide sans journal (contrat de git) : Astra et Sol pour, Opus contre (promesse de l'ADR 0025). ~15–30 ms.
2. Pas de capture du projet avant une fusion (git ne vérifie que les chemins touchés) : ~20 ms.
3. Identité de contenu (FileId et ChunkId de 32 octets) : seul levier restant pour la taille (2 620 contre 1 822 Kio).
4. `room drop` d'une Room physique : l'ADR 0025 impose de tout relire et hacher ; sur une Room fraîche de 10 000 fichiers, ~10 s (antivirus à chaque première ouverture). git se fie aux métadonnées de l'index.

## Point courant - packs en ligne, création de Room, conseil s8 (Claude + conseil, 2026-11-09)

**Banc à 10 000 fichiers, format 6** (`1d651a6`, deux répétitions, machine chargée ; git dans la même zone) :

| | layrs | git |
| --- | --- | --- |
| `init` | 1,2–1,6 s | 43 s |
| `status` propre / 2 changements | 60–65 / 67–69 ms | 51–54 / 55–56 ms |
| `step` (2 fichiers) | 143–146 ms | 152–156 ms |
| création de Room (agent) | 2,63–2,67 s | 2,29–2,32 s |
| première lecture de tous les fichiers | 3,29–3,66 s | 2,95–3,23 s |
| `room step` de l'agent | 172–183 ms | 283–297 ms |
| fusion en avance rapide / 2e fusion | 157–168 ms | 73–80 ms |
| fusion 3 voies | 333–344 ms | 354–368 ms |
| suppression de la Room | 0,33 s | 1,35–1,57 s |
| Store après le scénario | 2 620 Kio | 1 822 Kio |

Depuis (`caf35b8`, `05dba21`, machine plus calme) :
- Les écrivains parallèles prennent les dossiers à tour de rôle : créer à 8 threads dans un seul dossier NTFS est ~3,7 fois plus lent (1 240 contre 334 ms pour 8 000 fichiers). Room virtuelle 2,52–2,60 → 1,97–2,11 s (git 2,29–2,32), Room physique 2,19–2,21 → 1,36–1,41 s, première lecture 3,28–3,49 → 3,06–3,31 s.
- Frontières du Store validées par la racine épinglée (une marche, puis des ouvertures relatives) : `status` ~42,3 ms contre git ~39,4 ms dans le même dossier.
- Reste : `status` (~3 ms ; le plus gros poste est le décodage de l'index du pack initial, 2,5 ms, inutile pour lire la vue — différer le décodage est une question de conception), fusions en avance rapide (décisions utilisateur), taille (identité de contenu, décision utilisateur ; SQLite ~420 Kio de structure fixe), staging à plat d'une matérialisation dans un dossier existant (même contention NTFS, touche au format de reprise).

## Point courant - performances et taille, « je veux tout » (Claude + conseil s2, 2026-11-07)

Demande : « Reprends sur tout, je veux tout » (fusions, taille, Rooms hybrides, toutes les commandes contre git).

**Mesures à 10 000 fichiers, format 6, même machine** (`bench_vs_git_samezone.sh`, deux répétitions, `c5c7f38` ; démarrer un processus coûte ~40 ms pour les deux outils)

| | début de séance | `c5c7f38` | git |
| --- | --- | --- | --- |
| `init` | 2,4 s | 1,2–1,5 s | 47–48 s |
| `status` propre / 2 changements | 84 ms | 68–70 / 76–77 ms | 52–53 / 58–61 ms |
| `step` (2 fichiers) | 207 ms | 146–158 ms | 153–157 ms |
| création de Room (agent) | 3,1 s | 2,56–2,57 s | 2,16–2,18 s |
| première lecture de tous les fichiers | 3,3 s | 3,29–3,45 s | 2,78–2,82 s |
| `room step` de l'agent | 248 ms | 254–309 ms (bruit, cinq essais) | 199 ms |
| fusion en avance rapide | 580 ms | 184–190 ms | 73–75 ms |
| 2e fusion | 576 ms | 179–184 ms | 79–88 ms |
| fusion 3 voies | 990 ms | 369–379 ms | 326–337 ms |
| suppression de la Room | 1,18 s | 0,43–0,48 s | 1,35–1,39 s |
| Store après `init` / après le scénario | 2 369 / 4 249 Kio | 2 049 / 2 600 Kio | 1 655 / 1 822 Kio |

**Commits**
- `6dd82a5` : `status` dans une seule session du Store ; vue CAS et registre d'identités mis en cache par session ; identifiants de contenu stockés en ligne (71 octets, copie sans allocation) ; décodeurs de caches plus rapides ; `status` sans seconde lecture des dossiers (comme git).
- `c8d01c9` : encodeur canonique direct des arbres ; validation des chemins sans allocation ; ouverture du Store 2,5 → 1,2 ms.
- `cc12eda` : sentinelle de changements dans l'hôte VFS (`ReadDirectoryChangesW`, comme le fsmonitor de git) : une capture de Room inchangée 90 → 9 ms ; moins de sessions par fusion et par capture.
- `c63a92f` : cache de projection v4 en deltas (1 650 → ~420 Kio après le scénario).
- `4b1f48f` : corrections du conseil s2 r1 ; réemploi d'un objet stocké par comparaison d'octets ; taille d'un fichier sans construire ses manifestes ; enveloppe canonique directe des arbres (lecture d'arbres 95 → 34 ms sans cache, fusion 3 voies 591 → 431 ms) ; journal VFS supprimé avec sa Room.
- `1dddf9e` puis corrections s2 r2 : matérialisation (voir les deux décisions ci-dessous).

**Conseil s2 r1** (Astra 7, Sol 8, Opus 8, `ship after fixes`) : corrections dans `4b1f48f` (sentinelle conclue par annulation et port de complétion ; contrôles du fournisseur sur le raccourci ; `status` relâche sa session avant de lire le dossier ; mémoires bornées à une entrée par chemin). Décisions :
- Mémoires de processus (validation des frontières du Store, en-tête WAL, chemin canonique de la base) : gardées. git ne fait aucun de ces contrôles ; chaque session cliente revérifie l'identité épinglée de la racine.
- B1 (format 6 par défaut) : voir `docs/plans/store-format-6.md`, section des bloquants.
- B2, décision 1 (transactions) : preuves et bail en une transaction (`prepare_and_claim`) ; métadonnées et clôture du journal en une transaction (`complete_in_transaction`, heure du bail avancée du temps écoulé). Un arrêt après cette validation ne laisse rien à réparer.
- B2, décision 2 (durabilité NTFS) : sur un seul volume NTFS (dossier et Store), les synchronisations de dossiers des phases sont remplacées par une barrière (fichier témoin vidé) avant la validation des métadonnées, comme le mode `batch` de git et les lots CAS. Le dossier de la tentative reste créé avec synchronisation, avant que le journal passe `applying`. Ailleurs, rien ne change.
- Fusion en avance rapide 255 → ~235 ms.

**Revue du conseil (session 2)**

| Membre | r1 | r2 | r3 |
| --- | --- | --- | --- |
| GPT-6 Astra (xhigh) | 7/10 | 8/10 | 8/10 |
| GPT-6.1 Sol (max) | 8/10 | 8/10 | 9/10, `ship` |
| Claude Opus 5.5 (xhigh) | 8/10 | 7/10 | 8/10, `ship` |

Corrections r3 : la clôture lit l'horloge des baux dans la transaction (horloge injectable, test d'un saut au-delà de l'expiration) ; le mode NTFS ordonné exige un volume local (un partage SMB qui annonce NTFS garde ses synchronisations, CAS compris) ; un fichier témoin impossible à supprimer ne fait plus échouer la matérialisation ; `complete_in_transaction` redevient interne, son test est dans la crate.

Après r3 (non relu par le conseil, selon la recommandation d'Opus r3) : un lot de matérialisation juge ses baux à l'heure où il a pris le Store en exclusif (personne ne peut les reprendre jusqu'à la validation), puis les prolonge à partir de maintenant ; la clôture les vérifie à son heure. Une réception dont l'import dépasse le bail (de l'ordre de 40 à 100 Go selon le disque) ne bloque plus. Pas de test qui fasse vraiment durer un import au-delà de 5 minutes.

**Conseil s3 (conception, Astra, Sol et Opus d'accord)** — brief `council-s3-design.md` :
- D1, caches du dossier de travail : garder les deux fichiers. L'index de changements omet un FileId par référence à la projection d'un arbre racine nommé, liée par un condensé (format LYRSIDX3 : arbre lié et condensé canonique de sa projection dans l'en-tête, un bit par entrée « FileId, longueur et bit exécutable pris dans la projection liée »). Seul l'encodeur décide d'omettre, en comparant à la projection que l'auteur a utilisée. À la lecture, une projection absente ou de condensé différent est reconstruite depuis les arbres du Store (jamais de re-hachage) ; si elle reste différente, seules les entrées sans FileId sont relues. Fusionner les deux caches (modèle git) a été écarté : le cache de projection sert tout le Store, et chaque Room répéterait les FileId. Gain attendu : environ 320 Kio à 10 000 fichiers.
- D2, coût fixe d'une session d'écriture : écrivain en `synchronous=NORMAL`, et chaque session finit par un point de contrôle `TRUNCATE` vérifié (aucun lecteur occupé, toutes les trames copiées, WAL vide) avant de rendre la main. Rien qui dépende d'une validation ne se passe avant ce point. Une seule connexion pour la validation et la création du journal. À mesurer : `locking_mode=EXCLUSIVE` (pas de `-shm`) et WAL persistant vidé. Écartés : lecteurs WAL normaux (tout le chemin de lecture change) ; durabilité paresseuse façon git (une fusion perdue puis un `room drop` durable perdrait le travail d'une Room).
- D3, alias ChunkId en préfixe : refusé. L'identifiant de manifeste et les réponses qui rendent des ChunkId devraient alors lire et hacher le contenu, et une corruption deviendrait une absence.
- D4, création de Room : fermeture de chaque fichier tiède juste après sa conversion (fait, `ce9c066`) ; ensuite, mesurer la contention par dossier et créer chaque fichier relativement à la poignée de son parent. Écrire les fichiers avant l'enregistrement de la racine reste possible, mais seulement avec une reprise d'une racine non enregistrée et non vide (preuve par l'identité de la racine).

**Fait depuis s3** :
- `86baba5` : D2. Les sessions d'écriture se terminent par un point de contrôle vérifié, avec `NORMAL`, verrou SQLite exclusif (pas de `-shm`) et WAL persistant vidé. Un COMMIT coûte 1,5–2,5 ms au lieu de 3,5–6 ms ; le gain total est faible (le point de contrôle reste à ~3,5 ms).
- `1f2b25c` : D1. Index de changements LYRSIDX3 lié au condensé de la projection : Store 2 924 → 2 592 Kio, index 524 → 204 Kio.
- `97912d0` : parcours des sous-dossiers de la racine en parallèle (parcours 7,3 → 3,4 ms). La vérification de la base de `status` par la version du fichier SQLite a été retirée après la revue s5 r1 (voir plus bas).
- `status` dans le processus : 42 → ~26 ms (git : 9,7 ms ; démarrer un processus coûte ~40 ms pour les deux).
- `acb4008` : Q1 et Q2 du conseil s4. Les fichiers sont stagés avant le journal, qui est créé directement `applying` (`create_and_claim`) : deux sessions exclusives par matérialisation au lieu de trois. Une session de lecture tenue pour les lectures d'un fast-forward : 7 ouvertures partagées au lieu de 11.

**Revue du conseil s5 r1** (Astra 7/10, Sol 6/10, Opus 7/10 ; tous `ship after fixes`). Corrections :
- `NORMAL` ne sert plus qu'aux sessions qui finissent par un point de contrôle vérifié (le lot et la session du journal de matérialisation) ; tous les autres écrivains, dont la reprise et les baux, restent en `FULL` (Sol P0, Astra et Opus P2).
- `status` relit de nouveau sa base après le parcours : la version du fichier ne prouve rien sur un volume à dates grossières ou si l'horloge recule (Astra et Sol ; Opus la jugeait saine avec une condition sur le volume).
- Le parcours n'ouvre un sous-dossier de la racine qu'au démarrage de sa branche (toutes ensemble dépasseraient la limite de descripteurs sous Linux) ; au plus quatre fichiers hachés à la fois toutes branches confondues.
- La matérialisation refuse si le dossier relu sous le verrou exclusif diffère de celui où elle a stagé (c'était une simple assertion de débogage).
- Journal `layrs.checkout-write.v2` pour les tentatives stagées d'abord : un binaire plus ancien le laisse en attente au lieu de chercher le dossier sous l'autre nom.
- Une reprise balaie aussi les dossiers de tentative du Worktree qu'aucun journal ne nomme.
- Un fichier du cache de projection n'est remplacé que si la projection reconstruite en diffère.
- Reporté : lier l'index à l'arbre racine de l'étape plutôt qu'à l'arbre de projection (performance seulement, en cas de pierres tombales).

**Revue du conseil s5 r2** (Astra 8/10, Sol 8/10, Opus 8/10 `ship`). Corrections (`b57f50b`) :
- Le nombre de branches parcourues en même temps est borné par la limite de fichiers ouverts du processus (hors Windows) : chaque branche garde au plus sa chaîne de dossiers, un lot de dossiers terminés et quelques fichiers (Astra et Sol P2).
- `room drop` balaie les dossiers de tentative du Worktree retiré qu'aucun journal ne nomme (Opus P3 : un arrêt pendant le staging d'un gros fichier laissait sinon des Go).
- Une entrée disparue entre le listing et son ouverture rend la passe instable (relancée) au lieu d'une erreur.
- Chaque branche crée au plus sa part des quatre hacheurs ; la reprise retrouve aussi le dossier préfixé d'un journal v1 marqué `worktreePrefixed` (écrit par `acb4008`, jamais publié) ; un test fige quels écrivains sont en `NORMAL`.

| Membre | s5 r1 | s5 r2 |
| --- | --- | --- |
| GPT-6 Astra (xhigh) | 7/10 | 8/10 |
| GPT-6.1 Sol (max) | 6/10 | 8/10 |
| Claude Opus 5.5 (xhigh) | 7/10 | 8/10, `ship` |

**Consolidation des packs du format 6 (ADR 0028, section 6)** — `9cd1f0b`, corrections `de9731c` (s6 r1), `533c089` (s6 r2), test `e918531` :
- Déclenchée à la fin d'un lot d'écriture du format 6, sous le même verrou exclusif, quand le Store compte 6 packs inscrits (le pack du lot compté seulement s'il en a publié un).
- Choix géométrique par palier de taille : le palier suivant est essayé si le plus petit pack est isolé. Chaque pack ajouté fait au plus deux fois le total du groupe. Le groupe reste sous la moitié des limites du lecteur et sous un budget de 64 Mio par consolidation (les paliers plus gros attendent un passage de maintenance explicite, à faire).
- Copie dans l'ordre des blocs : chaque bloc est décompressé une fois, chaque record est vérifié contre son identifiant puis copié tel quel. Un record que deux candidats contiennent est écrit une fois.
- Une transaction inscrit le nouveau pack et remappe les bitmaps, une Space à la fois. Chaque bit admis doit désigner la même identité dans l'ancien et le nouvel index ; le bitmap écrit est relu par sa clé et comparé octet par octet. Elle retire ensuite les anciens packs (chacun par séquence, nom et condensé). Les fichiers retirés sont supprimés à la réouverture exclusive suivante, dans la même session. Le VFS relit le catalogue quand un pack qu'il nomme a disparu, comme git.
- Un candidat illisible ou au bitmap mal formé est écarté, sans bloquer les autres. Il est noté dans `metadata/consolidation-skipped` pour que les sessions suivantes ne le retentent pas. Une erreur d'écriture du nouveau pack (disque plein) n'accuse aucun candidat.
- Mesuré : le scénario à 10 000 fichiers finit avec 7 packs au lieu de 20 ; Store 2 600 → 2 536 Kio ; chargement du catalogue de `status` 3,4 → 2,1 ms.

| Membre | s6 r1 | s6 r2 | s6 r3 |
| --- | --- | --- | --- |
| GPT-6 Astra (xhigh) | 7/10 | 8/10 | 6/10 |
| GPT-6.1 Sol (max) | 7/10 | 7/10 | 6/10 |
| Claude Opus 5.5 (xhigh) | 6/10 | 7/10 | 8/10 |

Corrections s6 r3 (`6c22006`) : notes de `metadata/` lues sans suivre de lien et remplacées par un fichier neuf (P0 d'Astra et Sol : un lien posé là faisait tronquer sa cible) ; budget décompressé de 256 Mio ; inscription Space par Space ; une erreur d'E/S en lecture ne met plus un candidat en cause ; pause d'une heure après deux échecs généraux. Rejeté : un index sur `pack_seq` (Sol P2 ; Opus l'accepte sans). Dernier tour : toutes les suites Windows vertes, poussé sur main (`47b7b36`), CI verte.

**Conseil s7 (conception)** :
- Q1, petits lots : les trois pour. Retenu le design d'Opus (image du pack dans une ligne SQLite, inscrite dans `cas_packs`) ; Astra et Sol le jugent crédible.
- Q2, capture incrémentale des Rooms : oui, mais seulement avec un observateur continu dont la complétude est prouvée (et un mémo d'arbres par dossier). À faire.
- Q3, relecture de `status` : (b) pour le seul `status` informatif (Astra, Sol ; Opus en repli) — fait (`47b7b36`) : ~57,5 → ~50,8 ms (git ~49 ms, machine chargée).

**Packs en ligne (s7 Q1)** — `c096e7b` puis corrections s8 r1 : voir `docs/plans/store-format-6.md`. Capture de Room : `cas.put_ordered` 18 → 4 ms, barrière sautée.

| Membre | s8 r1 |
| --- | --- |
| GPT-6 Astra (xhigh) | 8/10 |
| GPT-6.1 Sol (max) | 8/10 |
| Claude Opus 5.5 (xhigh) | 7/10 |

Corrections s8 r1 : barrière sautée en format 6 seulement (P1 d'Opus : un lot du format 5 peut réutiliser un pack dont la barrière n'a jamais tourné) ; scrub depuis un catalogue frais ; plafond borné à 1 Mio ; forme déclarée (`stored_inline`) et diagnostics à la ligne de la base ; les packs en ligne fusionnent entre eux à 8 au lieu de compter dans le seuil de 6 ; orphelins, déversement, comptage des images retirées ; aides de test derrière la feature `test-support`.

**Découverte de `status`** — `82cbe12` : chaque dossier épinglé une seule fois. Avant, un `status` parcourait 13 fois un chemin depuis la racine du volume (119 ouvertures) plus 15 `lstat` d'ancêtres. Le parcours d'épinglage refuse déjà un lien ou un point de reparse à chaque composant ; la découverte garde le dossier de travail épinglé et le Store est lié à sa racine épinglée. `status` dans le processus : 29,8 → 24,5 ms ; au mur, ~48,5 ms contre ~40 ms pour git (machine calme, 10 000 fichiers). Mesuré : une requête par chemin coûte ~0,35–0,45 ms sur ce poste (filtres antivirus et ProjFS), une ouverture relative presque rien ; démarrer layrs coûte ~24 ms, git ~33 ms.

**Conseil s4 (conception, brief `council-s4-design.md`)** :
- Q2, ouvertures du Store (unanime) : faire passer une session de lecture explicite à travers chaque phase sans interruption (découverte, capture avant le parcours, de l'état jusqu'au staging). Jamais tenue pendant une requête à l'hôte VFS, un parcours de dossier ou une ouverture exclusive du même processus ; un registre par processus refuse une ouverture exclusive pendant qu'une session partagée est vivante. Refusé : réutiliser une connexion ou un état entre deux sessions (la longueur et la date d'un fichier ne prouvent pas qu'il n'a pas changé).
- Q3, double validation (unanime sur le fond) : valider une seule fois, en gardant la capacité exclusive de la validation jusqu'au point de contrôle final, pour les petites matérialisations. Ailleurs, les deux validations restent.
- Q1, sessions d'une petite matérialisation : Astra et Sol proposent un chemin rapide sans journal ni sauvegarde, sous une seule session exclusive tenue pendant l'installation (le contrat de git en cas d'arrêt). Opus le refuse : la capture automatique avant opération enregistrerait l'état mélangé comme une étape, ce que git ne fait pas ; et le remplacement par renommage perd une modification faite entre la dernière vérification et le renommage, contrairement à la promesse de l'ADR 0025 (« aucune écriture utilisateur perdue ou déplacée sans preuve »). Opus propose de stager avant le journal, puis de créer le journal directement `applying` (deux sessions, ou une seule tenue avec Q3).
- Décision du président : faire d'abord ce que les trois acceptent, c'est-à-dire stager avant le journal (preuves d'identité inchangées), puis une seule session exclusive tenue (journal `applying`, phases, métadonnées et clôture), avec une seule validation. Le chemin sans journal revient sur une promesse de l'ADR 0025 : c'est à l'utilisateur de décider.
- Hors questions : la capture du projet avant la fusion (42 ms) est la plus grosse part restante. La supprimer (comportement de git) est une décision produit.
- À revoir au prochain tour : la vérification par version du fichier dans `status`. Les trois membres refusent la longueur et la date comme preuve de non-changement ; ici, elle est gardée par une marge de 100 ms et ne sert qu'à éviter une relecture dans un affichage en lecture seule.

**Reste à faire**
- Fusions : matérialisation restante (~100 ms : trois sessions d'écriture, chacune avec son point de contrôle WAL) ; double vérification du dossier dans `accept_merge`.
- Taille : passage du format 6 par défaut (pack illisible ; consolidation faite, maintenance explicite des gros paliers à faire) ; identité de contenu (FileId et ChunkId de 32 octets, décision utilisateur) ; arbres binaires.
- Création de Room : écriture et conversion des petits fichiers (~2,4 s).

## Point courant - format 6 intégré, conseil r2-r3 (Claude + sous-agent Claude + conseil, 2026-11-06)

Le format 6 (ADR 0028, phases 2-3) a été implémenté par un sous-agent sur `feature/store-format-6`, puis fusionné dans cette branche (`550edce`, `1a1733d`, `dfed343`). Il reste opt-in : `LAYRS_STORE_FORMAT=6`.

**Revue du conseil**

| Tour | Astra | Sol | Opus | Verdict |
| --- | --- | --- | --- | --- |
| r2 | 6/10 | 7/10 | 6/10 | `ship after fixes` pour les trois |
| r3 | 7/10 | 7/10 | 8/10 | `ship after fixes` pour les trois, sans P0 ni P1 |

Tous les points relevés ont été corrigés dans `dc50350`, `0420b92`, `406cba3`, `d911986`, `86cd279`, `e469171` et `4c85174`. Le cache d'arbres est désormais limité à la session sur le Store. Les audits relisent les métadonnées de pack sur le disque et vérifient chaque copie.

Bloquants avant de passer le format 6 par défaut (voir `docs/plans/store-format-6.md`) :
- **Pack illisible.** Un seul pack illisible bloque tout le Store. Il faut un filtre négatif par pack et une commande `layrs store repair`.
- **Consolidation (phase 5).** Chaque lot ajoute un pack.

**Mesures à 10 000 fichiers, même dossier, format 6, contre git**

| | Layrs (format 6) | git |
| --- | --- | --- |
| `init` | 1,9 s | 43,8 s |
| `.layrs` après `init` | 2 373 Kio | 1 655 Kio |
| Store après le scénario complet | 4 257 Kio (format 5 : 46,5 Mo) | 1 822 Kio |
| `status` | 91–101 ms | 50 ms |
| commit | 231 ms | 150 ms |
| espace agent | 2,8 s | 2,2 s |
| lecture complète | 3,5 s | 2,7–3,4 s |
| fusion en avance rapide | 480–550 ms | 74–89 ms |
| 2e fusion | 493–548 ms | 72–75 ms |
| fusion 3 voies | 922 ms | 316–362 ms |
| `drop` | 0,5–0,6 s | 1,3 s |

**Tests et validation**
- Suites complètes passées en format 5 et en format 6 : `store-local`, `client-core` `native_*`, CLI `native_rooms`.
- Clippy propre sous Windows et sous Linux (croisé, zig).
- La CI ne tourne que sur `main`.

**Reste à faire pour la taille**
- Caches du dossier de travail :
  - ne plus répéter le FileId dans l'index de changements quand il est égal à celui de la base ;
  - réduire le cache de projection (au plus 4 fichiers d'environ 410 Kio).
- ChunkId sur un préfixe vérifié : environ 270 Kio.
- Arbres binaires (phase 4).
- Consolidation (phase 5) et deltas (phase 7).

**Reste à faire pour les fusions**
- Lot validé deux fois.
- Bloc avant le journal sans span.
- Une vingtaine d'ouvertures du Store par fusion.

**À relever séparément.** Le budget de calcul par défaut de fsck est dépassé vers 100 000 fichiers, dans les deux formats.

## Point courant - taille (ADR 0028), Rooms hybrides, fusions (Claude + conseil r1 taille, 2026-11-06)

Demande : « fais moi tout : taille, rooms hybrides et fusions ».

| Point | Où en est-on | Mesures à 10 000 fichiers |
| --- | --- | --- |
| Rooms hybrides | Commité, `ec28317` | Création 1,31 s → 2,7 s (git 2,52 s) ; première lecture 17,8 s → 3,1 s (git 3,2 s) |
| Fusions | Commité, `34ba0c4` et `5c714e8` | 3 voies 1 040 → 805 ms (740 ms sans fichiers hybrides) ; 2e fusion 571 → 516 ms ; `status` après une fusion 217 → 103 ms |
| `status` | Commité, `027b13a` | 96–101 ms (git 58 ms) |
| Taille, phase 1 (caches) | Commité, `691b174` | Index de changements 1 415 → 528 Kio ; cache de projection 988 → 412 Kio |
| Taille, format 6 (phases 2-3) | Délégué, en cours | Voir ci-dessous |

**Rooms hybrides.** Les petits fichiers sont écrits à la population, puis convertis en placeholders synchronisés après la connexion.
- Budget : `LAYRS_ROOM_WARM_BYTES`, 256 Mio par défaut.
- Hydratation proactive par `CfHydratePlaceholder` mesurée et rejetée : 72 s avec un thread, 9,4 s avec 8.

**Fusions.**
- Cache d'arbres vérifiés par processus.
- Une capture inchangée ne reconstruit pas ses arbres.
- La projection du résultat d'une fusion est mise en cache.
- Mémo de projection sans ouverture du Store.

**`status`.** Il rafraîchit les entrées « racy » de l'index, comme git.

**Taille, format 6.**
- Le conseil r1 est d'accord sur la direction ; deux désaccords sont tranchés dans l'ADR 0028 (identifiants seulement dans l'index, blocs zstd de 16 Kio).
- Spécification : `docs/plans/store-format-6.md`.
- Implémentation déléguée à un sous-agent, dans un worktree, sur la branche `feature/store-format-6` : schéma v3, packs v2, enregistrements « fichier ». À relire, avec un tour de conseil, avant la fusion.

**Reste.**
- `step` : 293–321 ms (git 162 ms).
  - Le format 6 doit retirer les 50 ms d'objets séparés.
  - Les arbres complets sont reconstruits (26 ms) ; à remplacer par un cache-tree.
  - `validate.prefetch_tree_files` prend 25 ms.
- Fusions : lot validé deux fois (`materialize.rs:383` et `repository.rs:405`) ; bloc avant le journal sans span (environ 60 ms) ; ouvertures du Store (20 par fusion).
- Création hybride : écriture de 1,2 s et conversion de 1,0 s. Paralléliser davantage, ou lire la source par lot.
- La CI ne tourne que sur `main`. La branche est poussée jusqu'à `691b174` ; pas encore fusionnée sur `main`.

## Point courant - fichiers de toute taille (ADR 0027) puis performances vs git, conseil r4 (Claude + conseil, 2026-11-05)

Demande : « ajoute le support des fichiers de n'importe quelle taille, PUIS ... dépasser git sur toutes les commandes sans faire de tradeoff que git ne fait pas ». Rien de commité.

Phase 1 (fichiers de toute taille), détails dans `docs/decisions/0027-fichiers-de-toute-taille.md` :
- découpe adaptative : 1 Mio jusqu'à 1 Gio, puis puissance de 2 de longueur/1024, au plus 16 Mio (`fixed_chunk_bytes`) ;
- plafonds 1/4 Gio levés partout dans le chemin natif ;
- manifeste « prouvé » (pas de re-hachage à l'admission) ;
- présence de morceau et d'arbre lue sur les métadonnées ;
- le spool oublie les morceaux déjà admis, et la capture filtre ceux des gros fichiers de base ;
- purge sûre des sauvegardes de matérialisation (dossier épinglé, sceau, Windows `open_immutable_file`) ;
- cache des morceaux d'hydratation ;
- distant : un fichier seul est hors budget, mais la pagination au-delà d'environ 156 Gio reste à faire ;
- serveur : budget d'amplification 4 Gio au-delà de 64 fois les octets distincts.

Le parcours complet passe à 1,5 et 5 Gio (`scratchpad/bigfile_e2e.sh`). Comparaison avec git sur 1,5 Gio : commit d'une édition 3,4 s contre 47 s ; `.git` 3,0 Gio contre 1,5 Gio pour le Store.

Conseil r4 (Astra 6/10, Sol 6/10, Opus 7/10, tous « ship after fixes ») : constats et décisions dans `D:/Layrs/tmp/council-r4/SYNTHESIS.md`, tous les constats confirmés étant corrigés.

Phase 2, gains, à chaque fois sans changer les garanties :
- cache persistant de projection par arbre racine (`scm_v1/projection_cache.rs`, binaire, blake3, recalcul de contrôle 1 fois sur 64, à chaque fois en debug) ;
- mémo de `projection_tree_id` par empreinte ;
- `status` en une session (`status_report`) ;
- validation des limites du Store une fois par processus ;
- schéma SQLite validé une fois par état de fichier, et `mmap` sur les connexions ;
- index de changements binaire (l'ancien JSON reste lu) ;
- handle par pack ;
- regroupement des longueurs, des présences et des manifestes admis ;
- Weave : fichiers des arbres durables non revérifiés ;
- scan : moins de travail par fichier, pas de revalidation quand le résultat égale la base ou la passe précédente.

Mesures à 10 000 fichiers (avec `LAYRS_PERF`, binaire r23) :

| commande | git | r16 | r23 |
| --- | --- | --- | --- |
| init | 43,3 s | 6,6 s | 5,9 s |
| status | 58 ms | 481 ms | 90 ms (88 ms sans perf) |
| commit 2 fichiers | 163 ms | 740 ms | 318 ms |
| nouvelle Room | 2,19 s | 2,40 s | 1,28 s |
| lecture complète juste après | 2,64 s | 19,8 s | 14,1 s |
| commit en Room | 252 ms | 593 ms | 376 ms |
| fusion en avance rapide | 85 ms | 1,79 s | 575 ms |
| fusion 3 voies | 369 ms | 2,31 s | 1,06 s |
| suppression de la Room | 1,39 s | 2,88 s | 2,9 s |
| Room physique | 2,19 s | 12,1 s | 9,1 s |

Mesure M3 (hydratation parallèle) : 8 lecteurs prennent 51 s contre 15 s en séquentiel, alors que l'hôte ne cumule que 1,9 s. Le plancher CFAPI, noyau et antivirus est d'environ 1,3 ms par fichier, et le préchargement parallèle est contre-productif ici. La « lecture complète » d'une Room purement virtuelle ne peut donc pas battre git sans décision produit : écrire les petits fichiers à la création, ou réchauffer en fond.

Tests : suites core, store, store-local, client-core, cli, vfs et weave vertes, à part les échecs déjà présents avant (`projection_recovery_routing` ×2, `scm_admission::history::worktree_cas...`). Les recettes natives VFS ne sont pas relancées.

Conseil r5 (décisions D1 à D6, synthèse dans `D:/Layrs/tmp/council-r5/SYNTHESIS.md`). Appliqué depuis :
- D1 : sondes CFAPI de vérification avant `drop` parallélisées ;
- D4 : capture en une passe pour `step`, `status` et `init` ; les vérifications avant suppression ou écriture gardent deux passes ;
- D3 en partie : un bail par worktree pour les nouvelles matérialisations (marque `leaseScope`), et plus de `lease.force()` redondant ;
- `drop` : désenregistrement et suppression par un processus détaché, lié à l'identité du dossier (P0 d'Astra) ; jamais de suppression sur place, le `drop` échoue si le dossier est occupé (P1 d'Opus) ; une nouvelle Room du même nom prend `nom-2` tant que l'ancien dossier existe.

Mesures r27 (avec `LAYRS_PERF`), contre git : `drop` 626 ms contre 1,39 s ; avance rapide 514 ms contre 85 ms ; 3 voies 1 058 ms contre 369 ms ; `commit` 293 ms contre 163 ms ; commit en Room 448 ms contre 252 ms ; `status` 90 ms contre 58 ms ; Room physique 10,6 s contre 2,19 s ; lecture complète 15 s contre 2,6 s. Les suites de tests sont vertes, à part les 3 échecs connus.

Décisions de l'utilisateur (5 novembre 2026, après r5) :
1. **Rooms virtuelles hybrides** : à la création, les petits fichiers (64 Kio au plus, avec un plafond total configurable) sont écrits physiquement, puis convertis en placeholders hydratés et in-sync ; les gros fichiers restent virtuels.
2. **`room merge` garde la capture** du travail non enregistré, accélérée par une énumération complète comparée au dernier step accepté de la Room, sans reconstruire de snapshot (D5).

Banc r28 contre git, 10 000 fichiers, médiane de 2 passages sans `LAYRS_PERF` (`D:/Layrs/tmp/vsgit/vsgit-r28.tsv`) :

| commande | git | Layrs r28 |
| --- | --- | --- |
| init | 44,7 s | 4,5 s |
| status | 52 ms | 90 ms |
| status, 2 changements | 55 ms | 122 ms |
| commit | 152 ms | 300 ms |
| nouvel espace | 2,24 s | 1,14 s |
| lecture complète | 2,89 s | 15,7 s |
| commit en Room | 250 ms | 444 ms |
| avance rapide | 79 ms | 570 ms |
| 2e agent | 74 ms | 536 ms |
| 3 voies | 354 ms | 1,03 s |
| `drop` | 1,43 s | 0,46 s |
| Room physique | 2,24 s (worktree) | 9,9 s |

Ordre prévu :
1. D2, chemin physique « dossier neuf », qui servira aussi au mode hybride ;
2. mode hybride, après la mesure M4 (`CfConvertToPlaceholder` ou transfert proactif) ;
3. D5 ;
4. packs pour les petits lots, avec regroupement automatique (équivalent du `gc`) ;
5. cache-tree (arbres reconstruits seulement pour les dossiers modifiés) ;
6. `status` minimal ;
7. sessions et transactions de matérialisation (reste de D3).

Le 5 novembre 2026, à la demande de l'utilisateur, tout le travail est commité en 9 commits thématiques (`c0e53e6..155a954`), la branche est poussée, puis fusionnée dans `main` (`5a4cbac`, merge `--no-ff`, poussé).

CI « Trust gates » de `main` amenée au vert (`c199968`, run 42097388834, Linux et Windows), à la demande de l'utilisateur, avant de reprendre les performances. Corrections, toutes poussées en avance rapide sur `main` et sur la branche :
- **Portes statiques** :
  - `rustfmt` et `clippy -D warnings`, les lints locaux étant autorisés sur l'élément quand une correction réécrirait la logique ;
  - découpe des 8 fichiers de plus de 2 000 lignes sans changer les chemins des tests ;
  - les JSON de preuve de `docs/continuation` sont exclus du contrôle de taille, comme les lockfiles.
- **Linux** :
  - `provider::checkout_journal` est passé en `cfg(windows)` ;
  - les éléments atteints seulement par Windows sont autorisés en `cfg_attr(not(windows), ...)`, vérifiés par un clippy croisé Linux (recette en mémoire `layrs-linux-crosslint`).
- **Tests obsolètes depuis a4c3551 ou b52b95a**, alignés sur le comportement voulu :
  - la récupération refuse un journal chaud partagé ;
  - liaison Worktree des vues ;
  - slugs de Space par contexte ;
  - un rebase partiel sous catalogue de Lens laisse le choix explicite ;
  - bannière « Aucun changement en attente de capture » ;
  - corps de réponse lu seulement en cas d'échec.
- **Rooms** : sous Unix, le ctime révèle une édition à stat identique ; seul Windows garde l'assertion « invisible pour `step` ».
- **CI et paquet** : le worker Lens est construit avant `test:tauri` et avant le paquet MSI, et le worker copié dans `resources` est exclu de l'empreinte des sources.

D2 implémenté dans `scm_v1/materialize/fresh.rs` (5 novembre 2026, pas encore commité au moment de la rédaction). Une première matérialisation dans un dossier qui ne contient que `.layrs` suit ce chemin :
- **Journal** : il est écrit dans le schéma `layrs.fresh-checkout.v1` et liste les entrées de premier niveau. Le dossier est revérifié vide sous le journal.
- **Création** :
  - les dossiers sont créés en séquence, sans sync ;
  - puis les fichiers sont créés en parallèle, de façon exclusive, relativement au handle parent ; chaque handle ne partage que la lecture et les octets sont prouvés par `VerifiedWriter` ;
  - chaque fichier est horodaté par une seule requête `FileBasicInfo`, puis fermé sur son fil, de sorte que l'analyse antivirus à la fermeture se chevauche avec les autres écritures.
- **Contrôle final** : chaque dossier est énuméré et comparé aux horodatages, avec la même confiance que l'index de changements. `drop` lit toujours chaque octet.
- **Commit** : la synchronisation des dossiers précède le commit des métadonnées.
- **Reprise** : si le commit n'a pas eu lieu, tout sauf `.layrs` est mis en quarantaine par renommage dans le dossier frère `.<nom>.layrs-abandoned-<tag>`, jamais supprimé, et la création peut reprendre. Si le commit a eu lieu, les fichiers sont gardés.

Écarts par rapport aux conditions du conseil r5 :
- **Condition 5** : clôture du journal et commit restent deux transactions. La reprise décide par l'Operation active, donc un checkout commité n'est jamais mis en quarantaine.
- **Condition 4** : les handles ne sont pas tenus jusqu'au contrôle final. L'énumération confronte les horodatages pris par le handle d'écriture. À faire valider par le conseil.

Lecture Store parallèle : `SharedStagingStore` devient un pool, une lecture et donc une connexion SQLite par écrivain. Avant, les requêtes de manifeste se sérialisaient sur un seul `Mutex<Connection>`. Résultat à 10 000 fichiers pour une Room physique : 9,4 s, puis 2,6 s ; `git worktree add` prend 2,5 à 2,8 s, dans le même dossier.

**Mesures faussées par l'emplacement** (micro-banc `createbench`, création, écriture et fermeture de 10 000 fichiers sur 8 fils) :

| situation | durée |
| --- | --- |
| exécutable sur C: | 1,1 s |
| même exécutable lancé depuis `D:\Layrs\tmp\roomtest` | 3,9 s |
| dossier cible dans `%TEMP%` | 1,1 s |
| dossier cible dans `AppData\Local` ou dans le dossier personnel | 3,3 à 3,5 s |

L'antivirus est la cause probable. Tous les bancs précédents lançaient layrs depuis D:, et créaient les Rooms par défaut dans `AppData\Local\Layrs\rooms`, alors que les worktrees git étaient dans `%TEMP%`.

Banc r34z, layrs lancé depuis C: et Rooms dans le même dossier que les worktrees git :

| commande | git | layrs r34z |
| --- | --- | --- |
| status | 58 ms | 110 ms |
| commit | 162 ms | 333 ms |
| nouvel espace virtuel | 2,52 s | 1,31 s |
| lecture complète | 3,2 s | 17,8 s |
| commit en Room | 272 ms | 500 ms |
| avance rapide | 84 ms | 675 ms |
| 2e agent | 82 ms | 794 ms |
| 3 voies | 374 ms | 1,33 s |
| `drop` | 1,59 s | 0,54 s |
| Room physique | 2,52 s (worktree) | 3,15 s |

La lecture complète reste lente une fois l'emplacement corrigé : le mode hybride est confirmé. Ordre suivant :
1. mode hybride ;
2. fusions (D5) ;
3. commit et `status` ;
4. à soumettre à l'utilisateur : l'emplacement par défaut des Rooms, à côté du projet comme les worktrees git.

Mesure M4 partielle, écriture physique de 10 000 petits fichiers (node, Defender actif) : 4,2 s avec un écrivain, 2,9 s avec 4, 2,5 s avec 8 ; un `grep` ensuite prend 0,85 s. `git worktree add` prend 2,24 s.

Conception D2 et hybride (à implémenter) :
- **Payload** : `materialize`, avec le schéma `layrs.fresh-checkout.v1`. Le schéma SQL interdit un nouveau `recovery_type` ; un ancien binaire voit un journal en attente et refuse, donc l'échec est fermé.
- **Étapes** :
  1. Le journal est écrit avant tout effet.
  2. Le dossier est revérifié par le handle épinglé : il ne contient que `.layrs`.
  3. Les dossiers sont créés en séquence, puis les fichiers en parallèle avec `CREATE_NEW`, relativement aux handles parents, et vérifiés par `VerifiedWriter`.
  4. Les handles restent ouverts en lecture seule partagée jusqu'au contrôle final par énumération.
  5. Ready et la clôture sont écrits dans une même transaction.
- **Reprise** : quarantaine par renommage dans un dossier frère `.<nom>.layrs-abandoned-<n>`, sur le volume de la Room, jamais par suppression.
- **Hybride** : pendant la population VFS, les entrées de 64 Kio au plus, sous un plafond total, sont écrites physiquement, puis converties avec `CfConvertToPlaceholder` et `MARK_IN_SYNC`, avec l'identité du catalogue. Il faudra une variante d'audit pour ces placeholders hydratés, puis rejouer les recettes VFS natives.

Reste à faire, par écart :
- Room physique : chemin « dossier neuf » (B5) ;
- `drop` : vérification complète, décision ADR 0025 à revoir avec le conseil ou l'utilisateur ;
- fusions : capture de la Room par l'hôte (environ 170 ms), sessions de Store, transactions groupées (B1) ;
- commit et commit en Room : écritures CAS d'objets séparés et deux passes de scan ;
- `status` : environ 30 ms de trop.

## Point precedent - packs CAS (ADR 0026) et priorites du conseil r3 (Claude + conseil, 2026-11-05)

Decision utilisateur 2026-11-04 ("let's go") : packs pour les petits objets, ADR 0026 revisee apres le conseil r2 (Astra, Sol, Opus : go avec changements).

Fait (non commite) :
- Format de Store 5 (`PACKED_FORMAT_VERSION`) cree par `init` ; 2 encore lu et cree avec `LAYRS_STORE_FORMAT=2` ; reprise d'init garde la version du marqueur ; 3/4 (projection) et 6+ restent `FutureFormat`. Les capacites (`NativeStoreOpen/Audit/Exclusive`) portent la version validee (`format_version()`, `packs_enabled()`).
- `scm_v1/cas/packs.rs` : fichier `objects/packs/pack-<32 hex>.lpk` (en-tete, enregistrements autodescriptifs, index trie, pied avec blake3 en-tete+index) ; index valide comme entree non fiable (bornes, tri strict, plages sans chevauchement, 64 Kio/objet, 1M entrees, 64 Mio d'index, 32 Mio d'enregistrements) ; lecture exacte d'un membre (en-tete d'enregistrement compare a l'entree) ; catalogue par processus (id du Store + identite de la racine) ; scrub membre par membre et, sans index valide, par parcours des enregistrements.
- `put_ordered` : un `Packer` par lot (63 objets en memoire, flux vers staging a partir du 64e, nouveau pack au-dela de 32 Mio) ; moins de 64 nouveaux petits objets = fichiers separes (publies en parallele) ; sinon scellement (index + pied, vidage B ou sync A, relecture complete comparee aux empreintes), renommage sans ecrasement, identite verifiee, puis seulement l'etape de durabilite du lot. Reutilisation d'un membre seulement s'il relit les octets attendus (A : pack et `objects/packs` synchronises en fin de lot). Inventaire des packs une fois au debut du lot.
- Lectures (`read`, `verify`, `contains`, `stream_verified_chunk`, nouveau `locate`) : packs charges, fichier separe, puis packs publies depuis ; premiere copie qui se verifie ; un fichier separe deja diffuse au flux reste definitif. `read.rs` et `fsck` passent par `locate`. `insert_object_rows` refuse une ligne existante differente. Points de faute `BeforePackPublication`/`AfterPackPublication`.
- Tests : `tests/scm_cas_packs.rs` (seuil 63/64, doublons + gros objets separes, reutilisation A et B, membre abime isole et nomme par le scrub puis reecrit, index invalide / pack tronque, Store format 2 sans pack, `per-object`, pannes avant/apres publication puis reutilisation du pack orphelin) ; 5 tests unitaires du format.

Mesures (meme jour, binaires geles `D:/Layrs/tmp/roomtest/layrs-14.exe` / `layrs-15.exe`, profils `D:/Layrs/tmp/packs-r15/`) : init 10k 51.9/50.9 s -> 15.5/14.8 s ; publication CAS 33.8 -> 2.0 s ; `.layrs/objects` 20 013 fichiers / 54 Mo -> 10 fichiers + 1 pack / 9 Mo. Reste dans init : validation des admissions 8.5 s, capture 2.4 s, insertion des lignes 1.1 s. Benchmark Rooms 10k (`bench-rooms-r15.tsv`) : commandes inchangees, 1er grep virtuel 23.0 -> 19.7 s (hydratation depuis le pack).

Tests : `layrs-store-local` complet vert hors l'echec preexistant `scm_admission::history::worktree_cas...` (une regression trouvee et corrigee : un Store de projection format 3 doit rester `FutureFormat`) ; client-core/cli/vfs/weave verts hors les 2 preexistants `projection_recovery_routing` ; recettes VFS natives 10/10 ; `native_rooms_virtual` 2/2.

Comparaison avec git 2.45.1 (configuration par defaut, meme machine et jour, mediane de 3 ; scripts `bench_vs_git.sh` / `bench_texture.sh`, donnees `D:/Layrs/tmp/vsgit/`, page `D:/Layrs/tmp/vsgit/layrs-vs-git.html`). 10k : import 13.1 s contre 43.3 s ; nouvel espace d'agent 2.0 s (Room virtuelle) contre 2.2 s (worktree) ; texture 256 Mio x 4 versions : Store 261 Mio contre 1 Gio (256 Mio apres un gc de 10 s), commit 3.1 s contre 5.5 s. git gagne les commandes courtes : status 58 ms contre 466 ms, fusions 85-369 ms contre 1.4-2.3 s, commit 163 contre 718 ms ; Store 1.6 Mo contre 45 Mo (metadonnees SQLite) ; lecture complete d'une Room virtuelle 17.3 s contre 2.6 s.

Conseil r3 (consultatif, rien d'applique) : 3 membres d'accord sur les trois chantiers suivants, qualification native verte comme porte d'entree et de sortie de chacun : (1) gros fichiers de bout en bout (plafonds verifies : 1 Gio/fichier et 4 Gio/capture cote client, admission, transfert et serveur ; 100 000 entrees ; publication partielle 16 Mio ; resolution de fusion en memoire 64 Mio/256 Mio ; aucun producteur FastCDC) ; (2) parcours produit de clone partiel avec ACL par fichier (Opus : d'abord une ADR qui unifie le modele "absent ≠ supprime" des projections et des Rooms) ; (3) progression concurrente des Rooms (chunks installes hors de la section exclusive, seul le commit et le CAS de tete sous verrou). Opus place en tete la mise a l'abri du travail (commits, CI Windows) : decision utilisateur.

## Point precedent - Rooms (ADR 0025) : index de changements, capture VFS par enumeration, Weave paresseux, conseil r1 (Claude + sous-agent Claude + conseil, 2026-11-04)

Decisions utilisateur 2026-11-04 : nom produit Room (alias `loom`, `worktree`) ; Room virtuelle par defaut (`--physical` sinon) ; detection des changements "comme git" pour les Rooms PUIS pour le dossier principal (index par Worktree, `step/status --full` pour tout relire) ; garde-fou : verification complete avant `drop` et avant tout ecrasement.

Fait (non commite) :
- Surface `room new/list/path/status/step/merge/drop` (`layrs-cli/src/native_room.rs`, `args/room.rs`, `client-core/native_scm/rooms.rs`). Emplacement par defaut `%LOCALAPPDATA%\Layrs\rooms\<space>-<hash>\<room>` si meme volume, sinon `<projet>.rooms\`. Registre `.layrs/rooms/`. Worktrees retires (`retired-worktrees.json`). Merge : avance rapide (`fast_forward_layer`) sinon Weave.
- Index de changements `scm_v1/change_index.rs` (cle stat + `verifiedAt`, marge racy 100 ms NTFS/ReFS, 2 s ailleurs ; ecrit apres acceptation ; seede par la materialisation sous handle exclusif, par `init`, par la creation VFS). Scan `capture_streaming_trusting` (hook trust recoit la metadonnee listee). Actif pour tout dossier physique.
- VFS : checkpoint par enumeration `FileIdExtdDirectoryInfo` (etat CFAPI via tag de reparse, numero NTFS) sans ouvrir les fichiers ; placeholder in-sync = base meme hydrate ; `Vouch::Content` pour fichiers prouves ; `checkpoint_verifying` (handle attributs, etat in-sync natif) pour `drop` ; renommages acceptes dans la Room (ACK_RENAME si TARGET_IN_SCOPE) ; `.layrsignore` applique ; hachage en streaming (spool 1 MiB, 1/4 GiB) ; manifests lus a la premiere hydratation ; verrou partage du Store par plage (chemin valide une fois, fichier rouvert sans parcours des ancetres) ; hote : n'herite plus des pipes de l'appelant (bloquait `room new` sous un shell capture), redemarrage si protocole inconnu, libere ses handles avant de repondre a Stop.
- `drop` : une seule capture verifiante, dossier renomme a cote puis purge detachee (`__layrs-purge-dropped-room`), retry borne du renommage.
- Weave (sous-agent Claude) : `MergeFileSource` paresseux, `max_input_bytes` ne compte que les octets lus, arbres limites aux tetes/bases ; acceptation d'un merge propre par projection (`materialize_projection`), controle "dossier propre" en streaming/index.
- `status` : booleen par comparaison de projections ; diff detaille ne lit que les chemins changes.
- Perf : `LAYRS_PERF_OUT=<fichier>` (rapports JSONL, hote VFS par requete), spans d'hydratation `vfs.read_*`.

Mesures 10k (r7 -> r11, `D:/Layrs/tmp/council-r1/bench-table.md`) virtuel / physique : room new 16.7 -> 6.2-9.6 s / 19.9 -> 10.4-13.4 ; step apres grep 132 -> 1.0 s / 9.7 -> 1.9 ; merge avance rapide 11.5 -> 1.9 / 10.6 -> 2.4 ; Weave 2e agent 213 -> 3.8 / 160 -> 4.5 ; drop 11.6 -> 3.4-5.8 / 8.5 -> 2.8-3.1. 1k : toutes les commandes Room 0.4-1.8 s. Restent mauvais : 1er grep virtuel 10k 55.7 s (physique 0.9 s), init 10k 104-185 s.

Conseil r1 (Astra xhigh, Sol max, Opus xhigh ; brief `D:/Layrs/tmp/council-r1/BRIEF.md`) — decide et fait : population VFS par lots par repertoire ; capture unique au drop ; hote libere ses handles avant Stop + retry renommage ; capture unique au merge (`prepare_named_weave_after_capture`) ; pas de prechargement des chunks de base quand l'index couvre la base ; `status` sans lecture de contenu ; diff limite aux chemins changes ; `room step/status` physiques sans l'enveloppe `execute_workspace` ; index VFS non reecrit si inchange ; spans d'hydratation (mesure M1). Plus tard (mesurer d'abord) : sondes de verification du drop en parallele ; requetes SQL par lot (projections, ids de manifest) ; ouvertures du Store par commande (17-31) ; prechargement des manifests ; init (publication CAS : fsync fichier + 2-3 syncs de repertoire par objet ; toute coalescence = decision utilisateur, la durabilite decidee ne couvrait que les fichiers projetes) ; ecriture directe des Rooms physiques ; USN ; concurrence du verrou Store. Chantier structurel suivant : population VFS a la demande (seule voie vers un `room new` instantane ; distinguer "jamais peuple" et "supprime"). Rejete : confiance a l'index pour le drop, retrait du fsync CAS, hydratation complete anticipee, reecriture du Weave, demon global, exclusions antivirus, listes de chunks lues en SQL sans manifest.

Durabilite des lots CAS (decision utilisateur 2026-11-04, "A et B") : `NativeCasWriter::put_ordered` rend tout le lot durable a sa fin, avant tout commit de metadonnees (garantie inchangee au commit). A (partout) : plus de sync du repertoire `staging` dans un lot, creations de shards sans sync (`ensure_child_unsynced`), chaque repertoire touche synchronise une fois en fin de lot (8 threads). B (NTFS) : donnees de chaque objet ecrites sans vidage du cache disque (`NtFlushBuffersFileEx` FLUSH_FLAGS_NO_SYNC), aucun sync de repertoire (ordre du journal NTFS), un seul vidage final d'un marqueur cree apres toutes les publications (journal + cache disque). `staging`/`objects` epingles une fois par lot, chemins reverifies avant de declarer le lot durable. `LAYRS_CAS_BATCH_DURABILITY=per-object|coalesced` force un mode. `put` isole inchange. Init 10k 109.5 s -> 52.9 s (publication CAS 90 -> 34 s ; 81k syncs de repertoire -> 0) ; 3k 32.8 -> 20.7 s. Validation des admissions : plus de requete "deja admis" pour les objets du lot (valides en memoire). Reste : ~1.7 ms/objet de publication (creation, relecture, renommage, scan AV a la fermeture) -> packs plus tard ; validation 10-14 s a 10k.

Plancher CFAPI (mesure M2 du conseil, binaire experimental `D:/Layrs/tmp/roomtest/layrs-exp-memory.exe`, code retire) : hydratation servie depuis la RAM = grep 1k 1.5 s, 10k 11.5 s ; hydratation normale r14 a chaud 1.55-1.6 s (1k), 23 s (10k, objets froids) ; prechargement de tout le contenu depuis le CAS dans l'hote 0.47 s (1k) / 4.6 s (10k). Donc le cout fournisseur est ~0.1 ms/fichier a chaud, le plancher CFAPI ~1.2-1.5 ms/fichier (physique : 0.09 ms). Aller sous ce plancher = eviter le callback (pre-hydratation des petits fichiers en arriere-plan apres `room new`) : decision produit (disque/CPU). Benchmark final r14 (`bench-rooms-r14.tsv`) : init 10k 54-65 s (r12 107-109), commandes Room inchangees ; suite complete (-j 4) verte hors 3 preexistants ; natives VFS 10/10.

Tests : suite complete (5 crates, `--no-fail-fast`) verte hors 3 echecs preexistants (`projection_recovery_routing` x2, `scm_admission::history::worktree_cas...`) ; recettes VFS natives 10/10 ; `native_rooms` 4/4, `native_rooms_virtual` 2/2 ; 2 recettes natives mises a jour au nouveau contrat (in-sync hydrate = base ; deplacement capture). Flakes de fixtures pid+nanos corriges (`native_offline_tests`, `native_workflow`).

## Point courant - r214/r215 : contrat checkout "comme git", switch O(K), creation 10k 248s -> 11s (Claude seul + conseil Astra, 2026-11-04)

Decision utilisateur 2026-11-04 : durabilite "comme git" pour les fichiers projetes depuis le CAS (creation + switch) : plus de fsync par fichier (mesure machine : 10k fichiers avec fsync 8-12s, sans 1.9s). Chaque fichier stage est cree en refusant tout autre ecrivain (`create_file_denying_writers`), son identite + taille + mtime apres ecriture sont journalises (evidence `staged`). Recovery : avant commit, un fichier dont le contenu n'a pas survecu n'est mis en quarantaine (deplace, jamais supprime) que si identite, taille et mtime sont inchanges ; apres commit il est garde (modification de travail). Phase 1 (originaux) garde ses syncs. Echec de staging -> journal ferme "untouched" (`abandon_untouched`), pas de needs_repair ; pre-verification limitee aux fichiers de base deplaces (`verify_displaced_content`), les octets cibles sont prouves au staging (staging parallele 8 threads via `write_file_shared` + `SharedStagingStore`). Postcheck : fichiers installes valides via leurs handles (`is_at` chemin NT exact + taille/mtime) sans reouverture (evite le scan antivirus ~10s/10k) ; fermeture des handles en parallele.

Switch : pre/postcheck limites aux K chemins touches (`touched_paths_match` : empreinte de chaque original, nouveaux chemins libres, ancetres repertoires ; `touched_paths_installed`) au lieu de 2 scans complets ; chemins non touches jamais ecrits (edition concurrente = modification locale). Projection d'un Step mise en cache par processus (Step/Trees immuables, admissions jamais retirees) ; `admitted_chunk_ids_of_files` par lots de 256 ; journal en `prepare_cached`. CAS `put_ordered` : octets possedes en vol bornes a 8MiB (test tas 100MiB : 49MB < 64MB).

Conseil Astra (gpt-6-astra, lecture seule, autorise par l'utilisateur) : 2 problemes de correction releves et corriges (partage en ecriture des fichiers stages ; identite seule insuffisante pour deplacer en recovery). Publication par sous-arbre ecartee : Windows refuse de renommer un repertoire contenant un fichier ouvert (mesure), incompatible avec le postcheck par handle.

Mesure finale r215 (EXE `.layrs-local/validation/switch-optimized-r215/layrs-r215-release.exe` SHA `e8fac86dc186f669650786a465ebbbc8ef6f300be4a11fd3f43eda14abe36638`, `switch-optimized-r215-{1k,10k}-2026-11-04.json`). 1k : worktree create 1.36s, switch K=1 0.61, no-op 0.43, switch K=100 0.95, status 0.28, step K=1 0.49, step K=100 1.5, init 10.5. 10k : worktree create 11.0s (r213 42s, r208 248s), switch K=1 2.5 (r213 6.7), no-op 2.0, switch K=100 2.9 (7.4), status 1.6, step K=1 3.2, diff K=1 6.0, init 95s, 2 agents step 8.7/4.1. Tests r215 : seuls les 3 echecs preexistants ; 3 flakes de nommage de fixture (pid+nanos) corriges (`download_cache`, `native_offline`, `checkout_streaming`).

Reste : index stat/USN par worktree (capture = scan complet 1.2-1.8s a 10k, domine switch/status/step) ; creation : ~2.3s de renames depuis le staging (ecriture directe en place pour un worktree neuf = extension du journal) ; diff (recapture complete, 6s a 10k) ; concurrence agents (verrou Store exclusif) ; switch VFS O(K) ; 100k ; lot H.

## Point courant - lots perf r212/r213 : creation worktree, CAS et scans paralleles, revues 2 et 3 corrigees (Claude seul, 2026-11-04)

Mesures release (machine ~2x plus lente que le 03/11 : le binaire r210 gele met 4.1-4.9s pour status 10k ce jour contre 1.8s hier ; meme fixture, code actuel 2.5s). 1k `docs/plans/pme/measurements/switch-optimized-r212-1k-2026-11-04.json` : init 31.6s, status 0.44, step K=1 0.62, switch K=1 1.12, no-op 0.75, step K=100 3.1 (2.0 apres CAS parallele), switch K=100 2.0, worktree create 7.2 (3.7-4.2 apres staging parallele), 2 agents step 0.64/1.18. 10k `...-r212-10k-2026-11-04.json` : init 111s (399 avant CAS parallele ; echec en r208), status 2.8, step K=1 3.3, switch K=1 9.2 (switch K=100 7.5 -> 5.9 apres r213), step K=100 4.4, worktree create 37s (r208 248, r210 137), step concurrent 6.7/3.6. Profil creation 1k : ~20s -> 3.7-4.2s.

r212 (perf) : instrumentation `perf` attribuee (`fs.*@parent`) ; `verify_path_identity` chemin rapide Windows (GetFinalPathNameByHandle) ; pins CAS racine/objects caches 250ms pour lectures verifiees seulement (publication re-pin) ; `NativeStoreReading` (CAS + registre identite ouverts une fois pour le staging) ; materialize : pre/postcheck `matches_projection` (1 passe egale, sinon capture 2 passes), creation des fichiers stages en parallele, petits fichiers verifies (FileId) puis ecrits+fsync par workers avant toute installation, phase 2 : un parent par repertoire et syncs groupes apres la boucle (recovery decide chemin par chemin ; phase 1 inchangee) ; scan checkout : classification par l'enumeration, hachage parallele 4 threads (mesure : fichiers chauds sature a 2, fichiers neufs/antivirus jusqu'a 8) ; CAS `put_ordered` (installations concurrentes, resultats et points de faute dans l'ordre) ; `ensured_shards`.

Revue lot 2 (7 findings) corrigee : verif Store des fichiers de base deplaces (`verify_changed_content` 2 cotes, + `check_restore` avant capture auto, `receive`, reprise creation) ; fallback Store dans `write_file_from` quand le spool n'a pas le chunk (5 echecs `native_restore_preview` + 1 `native_concurrency`) ; `admitted_chunk_ids_of_files` (chunks de la base seulement, plus de chargement de tout l'Espace) ; manifestes LEFT JOIN + borne 1024 ; `MAX_BATCH_ADMISSIONS = MAX_BATCH_OBJECTS`. Test CLI ajoute `missing_base_chunk_refuses_switch_before_moving_current_files`.

Revue lot 3 corrigee (r213) : H1 un handle par repertoire pendant le scan (EMFILE Unix) -> scan par lots bornes (32 repertoires termines / 2048 fichiers : hachage puis recheck+fermeture enfants avant parents ; fenetre de stabilite reduite) ; parents installes bornes a 64 (sync anticipe). M1 chemin rapide identite : objet sans reparse ni suppression en cours, comparaison en chemins NT avec lettre resolue dans les DosDevices de l'appelant ; divergence documentee : tag non-lien (cloud/ProjFS) pose sur un ancetre apres le pin non recontrole. M2 accepte partiellement : la capture ne relit plus les copies Store des chunks connus (verification a l'usage : switch/restore/receive refusent avant journal) ; `check_restore` n'exclut plus que les fichiers absents du Store. L1 `put_ordered` : receiver possede par les workers (pas de blocage si tous paniquent) ; L3 budget total charge avant lecture ; L6 receive : seuls les chemins changes ; L2/L5 commentaires ; L7/L8 acceptes. Perf r213 : capture/diff acceptes en 1 passe si egaux a la base ; `clear_obstructing_directories` limite aux chemins changes (20k verifications -> 0 au switch 10k).

Tests r212 et r213 : seuls les 3 echecs preexistants connus (`scm_admission::history::worktree_cas...`, `projection_recovery_routing` x2) ; CLI 100% vert. Flake corrige (test seulement) : fixture `tests/download_cache.rs` nommee pid+nanos (collision en parallele, 1/8 runs) -> + compteur. Non commite (sur demande).

Mesure finale r213 (EXE gele `.layrs-local/validation/switch-optimized-r213/layrs-r213-release.exe` SHA `d1eb29513cdc23b2bf74cde71a594bbcefab9e9fb2f56fe6f501a9196695cc7b`, `switch-optimized-r213-{1k,10k}-2026-11-04.json`). 1k : init 10.1s, status 0.29, step K=1 0.50, switch K=1 0.88, no-op 0.59, step K=100 1.5, switch K=100 1.4, worktree create 4.0, 2 agents step 0.56/1.03. 10k : init 106s, status 1.6, step K=1 3.4, switch K=1 6.7, no-op 4.3, step K=100 4.7, switch K=100 7.4, worktree create 42s (cout par fichier, antivirus), 2 agents step 3.6/6.8, diff K=1 7.8 (a profiler).

Reste : index stat/USN par worktree (status/switch 10k encore O(N) : 2-3 scans + projections SQL x3) ; concurrence agents (verrou Store exclusif, CAS View global) ; switch VFS O(K) ; ~30 `open_store` par creation ; mesure 100k ; lot H natives Restore non-C14 ; 3 echecs preexistants.

## Point precedent - optimisations switch/step : capture O(changement) (Claude seul, 2026-11-04)

Mesure r209 (`docs/plans/pme/measurements/switch-optimized-2026-11-03.json`) 1k : switch 23.5->2.1s, step K=1 25.7->4.6s, status 3.6->0.6s, worktree create 46.8->26s ; 10k fonctionne (bornes batch relevees : MAX_BATCH_OBJECTS 262144, visites admission 524288/1048576, spool 262144 ; test budgets aligne) mais restait O(N). Suite (r210) : `file_length_metadata` lit `file_manifests.file_byte_length` (1 requete, plus de lecture d'objet manifeste) ; `ScmReader::file_manifest_ids_metadata` et `object_admitted_metadata` (defauts = verification complete ; natifs SQL, `prepare_cached`) ; `admissible_manifest_for_file` et `validate_admission` decident l'admission durable par metadonnees AVANT de lire l'objet ; capture : chunks deja admis non mis en spool (`admitted_chunk_ids` charge avant le hachage, sans verrou pendant le scan) et batch filtre aux objets non admis (`retain_new_objects`). Resultat step K=1 : 1k 0.81s, 10k 2.15s (dont scan 1.55s) ; status 10k 1.76s. Mesure complete r210 en cours. Reste O(N) : scans de l'arbre (index stat a faire), projections (3x lecture SQL N lignes), creation de worktree (copie physique).

Revue independante perf : aucune perte/publication non verifiee. Corrige : H1 test `native_workspace` adapte (projection metadonnees ; restore avec contenu falsifie refuse sans ecrire) ; M1 `verify_changed_content` (client-core projection.rs) verifie completement AVANT journal/registration le contenu des seuls fichiers ecrits (switch/restore/rewrite via apply_projection_transition, creation worktree), en excluant les fichiers fournis par les objets crees de la transition (regression `native_rewrite` Missing("File") corrigee) ; M2 fast path CAS fsync du shard une fois par vue ; commentaires layout/lease/admission corriges.

Tests : store-local lib 509/509 ; layrs-store OK ; client-core/cli : tous les echecs restants expliques. Preexistants (non lies, codes inchanges) : `scm_admission::history::worktree_cas_can_advance_only...` (InvalidOperationHeadsTransition, echoue aussi sans changements 4/5), `projection_recovery_routing` binding+recovery (rollback journal chaud refuse volontairement depuis 34ec205/b52b95a, tests obsoletes comme r4/provider_schema). Environnement : tests paralleles (lib client-core 12, compaction, worktree_safety, maintenance passent en serie), CLI `local_data_safety` = MAX_PATH du prefixe TEMP de test (passe avec TEMP par defaut). Ne pas utiliser de prefixe TEMP long pour les tests CLI.

Mesure baseline release r208 1k fichiers (`docs/plans/pme/measurements/switch-baseline-2026-11-03.json`, script `scripts/measure-switch.mjs`) : step K=1 25.7s, switch 23.5/24.3s, no-op 17.3s, worktree create 46.8s, 2 agents step 60.6/30.9s ; init 10k FAIL `combined canonical object count exceeds 16384` (limite batch). Instrumentation opt-in `layrs_store_local::perf` (LAYRS_PERF=1 -> JSON stderr du CLI). Profil switch 1k : verify_path_identity 107k appels 19s (connection() par requete SQL), file_availability 5304 (re-hash CAS 5x/fichier) 15s, validate_admission re-parcourt tout le graphe deja admis 2x/batch ; step : cas.put stage+fsync de TOUS les objets meme existants (14k dir_sync).

Corrections (non commit, tests en cours) : (1) projection checkout par metadonnees `file_length_metadata` (contenu verifie a l'ecriture) ; (2) verification identite implicite `NativeStoreOpen::connection()` et `LeaseKeeper::tick()` limitee a 1/250ms (bind, force et appels explicites inchanges) ; (3) `cas.put` reutilise un objet publie identique octet a octet sans staging/fsync (meme durabilite que l'ancienne branche existante) ; (4) `ScmReader::file_present_metadata` (defaut = verification complete ; natifs = metadonnees) pour `file_has_verified_manifest` ; (5) `validate_admission` s'arrete sur objets admis durablement (seul writer `insert_admissions` apres validate_batch ; fsck graphs verifie l'invariant). Resultat 1k : switch 35s -> 4.1-4.7s, step 33.6 -> 4.8s, status 3.7 -> 0.7s. A faire : tests suites, revue independante, mesure 1k/10k (corriger limite batch init 10k), puis index stat, concurrence, VFS.

## Point precedent - Restore C14 complet qualifie r208 ; priorite produit : switch de branche et worktrees par agent TRES rapides (Claude seul)

r208 lot G PASS : plain C14 3128.5s, companion 3099.8s (phases ready-version, capture-version, product-surface PASS). Lot H (5 natives Restore non-C14) differe : chemins quasi inchanges, a relancer apres optimisations. Demande utilisateur 2026-11-03 : changer de branche et avoir une branche par agent doit etre TRES rapide. Analyse `docs/plans/worktrees-fast-switch-analysis-2026-11-03.md` : switch O(N) (6-9 re-hash complets + 3 re-verifications CAS), aucun index stat, verrou Store exclusif + CAS global Space serialisent les agents, creation worktree copie O(N), VFS refuse le switch, C14 25-50 validations completes/operation. Ordre : (1) mesure release `layer use`/creation/2 agents 1k-10k-100k ; (2) corrections sures switch (une capture, post-verif K, projection metadonnees, identite chemin 1x/ouverture) + dedup validations C14 ; (3) index stat/USN ; (4) concurrence par worktree/WAL ; (5) switch VFS O(K). Etape 1 : build release CLI en cours.

## Point precedent - suite verte ; Restore C14 Version (Ready + Capture locale) et surface produit, natives r208 en cours (Claude seul)

Echecs journal chaud resolus : tests obsoletes depuis durcissements voulus (34ec205 maintenance owner RO sans rollback ; b52b95a catalogue famille sans rollback infere). `provider_schema::revisions::hot_populated...` attend maintenant le refus READONLY_ROLLBACK puis rollback test-only et verifie preservation ; `r4::retained_handle...` attend refus aussi pour binding de remplacement, catalogue inchange. Groupes r2/r4/provider_schema 38/38 PASS.

Restore C14 etendu : `RestoreSource::Version` accepte la generation Ready figee exacte (abandon des changements hors ligne) et `ProjectionVersion::Capture` locale post-Ready (route produit `PreviewRestore/ApplyRestore` qui n'accepte que des Versions) ; Remote et Capture importee/Ready refuses. Lecteur D7 : fichiers de base retenus par mapping fige (pas de claim captured-content). Trou routage produit corrige : `pending_publication` (Status) et `capture_history` (History) utilisaient des gardes/chargements non scopes refusant C14 (et History refusant tout admis) -> scopes. Native C14 : phases ready-version, capture-version, product-surface (state_with_selection, pending_publication, is_imported_capture, capture_history, preparations, compare_versions).

r208 EXE `63648353372f3970b4034512c3831ca3090a3c494013939225e6b8c0496ba77d` : suite non-ignoree 509/509 PASS (premiere suite verte). Lot G (natives C14 plain/companion 5460) en cours ; lot H (5 natives Restore non-C14) a suivre. r207 lance puis arrete volontairement (non concluant).

## Point precedent - tranche Restore C14 local TERMINEE et qualifiee (r204-r206, Claude seul)

r206 recovery PASS 782.2s (budget 1800). Disposition consolidee `.layrs-local/validation/c14-local-restore-disposition.json` SHA `b7c37127c719f7afa80913d0291bda81391244774a5d14b863c94ff8ac25c0fc` (script `D:/Layrs/tmp/summarize-c14-local-restore.py`) : 39 tests distincts, 0 echec ouvert ; produit identique r204/r205/r206 (seuls tests changes apres r204), chaque resultat lie a son EXE.

Suite non-ignoree complete r206 : 507 PASS / 2 FAIL / 582 ignored. Les 2 echecs (`tests::shared::r4::retained_handle_refuses_valid_family_marker_drift_before_hot_catalog_rollback`, `tests::shared::provider_schema::revisions::hot_populated_eight_owner_restores_committed_evidence_but_reclaims_nothing`, SQLITE_READONLY_ROLLBACK 776 sur rollback de journal chaud du catalogue famille) echouent identiquement sur EXE geles r21 (27/10), r56-r77 (28/10) et r202 : hors tranche, anciens ou environnementaux (KB5132402 02/11, KB5127936 03/11 candidats non prouves). A investiguer : recover_databases -> inspect(recover) -> ouverture catalogue avec journal chaud.

Non commite (regle : commit sur demande). Prochain programme (non commence) : perf lecteur C14 (index/cache des journaux et captures par snapshot), echecs journal chaud ci-dessus, C14 Restore Version/preparation/re-export, callers CLI/Desktop, suite L0-L9.

r204 lot C PASS : plain C14 Capture+Restore 2363.8s, companion 3537.0s (budget 3600 quasi atteint -> 5400/5460 pour les deux). Phases Restore toutes PASS (garde source, apply+dedup, Inverse octets jamais captures, Staged->cancel->retry, PathQuarantined->cancel refuse->resume, AfterCommit->replay, reouverture sans source). r205 (tests seuls vs r204, EXE `926d2b10...`) lot D : 3 restore_history PASS (dont canonical_history corrige), cancellation PASS 2174.4s (budget 2400), recovery timeout 720s apres iteration 0 PASS (assertion corrigee validee) -> 1800/1860. r206 (recovery seul change) EXE `44547d2bd750931cc768d57fd7f828e026f90452d896b3cae75e1c252600c16c` : recovery en cours.

Risque perf ouvert : cout du lecteur C14 (journaux et captures re-decodes a chaque validation, plusieurs validations par operation) ; operations Restore C14 ~100-150s en debug sur companion. Natives admises globalement plus lentes que le 27/10 (cancellation 929s -> 2174s).

r204 lot A 31/31 PASS. Lot B (fixtures sur C: obligatoires : sur D: `CfGetSyncRootInfoByHandle` echoue, premier lancement invalide sans effet) : frontier PASS 399s, export PASS 303s, native9 PASS 596s (budget 900 valide) ; recovery FAIL ligne 246 = test obsolete (budget `committed` d'un Restore non-C14 termine signale `Reservation` depuis l'ajout de ce motif admis apres r21 ; fixture verifiee : seul ce budget reste) -> test corrige (attend Reservation puis acces Exclusive qui solde) ; cancellation FAIL = timeout enfant 1200s en phase 6/10 (r22 27/10 : 929s ; ralentissement general des validations admises, aussi native9) -> budget 2400/2460. Test `restore_history::canonical_history...` corrige (fixture `inherited_segments` = vrai engagement vide). Ces 3 changements sont TESTS seulement ; r205 = r204 + ces tests, a compiler apres lot C. Lot C (C14 plain/companion avec phases Restore) en cours.

Restore C14 local (sources Capture post-Ready et Inverse local ; Version/preparation fermes) integre. Revue independante Claude (lecture seule) : 1 High corrige (id d'operation non canonique aurait cree un budget `restore:` illisible par le lecteur -> store bloque ; preflight refuse maintenant avant tout effet), Low corriges (clone snapshot seulement si journaux, message `shared quota reservation pending` pour budget etranger, dispatch C14 calcule une fois sans SQL). Low acceptes : borne journal conservatrice (+9 octets/temoin), cout lecteur (journaux <=64MiB re-decodes).

r203b (avant correctifs revue, non-natives) : 27/28 PASS ; echec `import_validation::restore_history::tests::canonical_history_includes_restore_only_preimage_and_shared_capture_operations` (`import ancestry commitment differs`) PREEXISTANT, identique sur EXE r202 gele, hors perimetre, a investiguer separement.

r204 gele `.layrs-local/validation/c14-local-restore-r204` EXE SHA `fc2a0a5021f13fb31458557f0ce5bbf58f0189585913d4d94159d8c494a04a91`, sources modifiees copiees dans `source/`. Runner generique `D:/Layrs/tmp/run-recipes.py <out> <recipes.json> [--keep-going]`. Lots : A 31 non-natives (r200 + Restore/historique + 4 nouveaux tests unitaires), B 5 natives non-C14 (recovery 780, frontier 540, export 660, cancellation 1260, native9 960), C 2 natives C14 plain/companion 3660 (phases Capture puis nouvelles phases Restore `c14_denial_start_tests/restore.rs` : garde source, apply+dedup, Inverse conserve octets jamais captures, prejournal Staged->cancel->retry, PathQuarantined->cancel refuse->resume, AfterCommit->replay, reouverture).

## Point precedent - r202 resume1 termine 2/4 natives ; Restore C14 local en source (Claude seul)

r202 bilan natif : plain C14 PASS 1386.9s (session55533) ; companion C14 PASS 1368.8s (resume1) ; native9 `managed_admitted_ordinary_edit_capture_restore_without_source` FAIL101 = timeout enfant 480s (pas d'assertion) : 1re restore a ~420s, 2e (Inverse) a ~459s, tuee pendant staging. Les phases P1 (CAS epoch+1, echec BeforeCommit, edit, refus, double cancel) ont allonge le test. Restore cancellation native non executee. Disposition `root-runtime-failure-disposition.json`, resultats resume1 SHA `e90a35b4ac1f1a384f14eac2c15717ee74e3ded40391811fb1bcaa25656ab613`. Fixture recipe-03 preservee.

Correction prevue r203 : budget native9 child480->900 / outer540->960 (test seulement). A requalifier sur r203 : native9 + restore cancellation + non-natives touchees.

Restore C14 local (design `D:/Layrs/tmp/c14-local-restore-design-r203.md`) en integration SOURCE, non compile : storage.rs (append force, visit avec base), format.rs (AccessPurpose::LocalRestore, non-C14 -> General), admitted_private_io.rs (handles restore/recovery/cancel + preflight_local_restore), content_scope.rs (denial_ready, preflight), restore.rs (garde source C14, resulting_changes), engine.rs (LocalRestorePlan, borne journal, financement avant reserve, settle C14 apres finish), nouveau read/c14_restore.rs + c14_capture.rs (historique/pending/prejournal/claims restore, budgets restore), retention.rs (MAX_RESTORE_BUDGET_BYTES, require_restore_staging). Restent : recovery.rs/cancellation.rs/admitted.rs, tests, compilation.

## Point precedent - r202 interrompu par redemarrage hote, reprise resume1 en cours (Claude seul)

Session55533 tuee par redemarrage manuel menu Demarrer 2026-11-03 01:24:51 (System User32 1074), puis redemarrage Windows Update 11:06, boot 11:08. Avant interruption : native plain C14 **PASS** sur r202 (1386.905s, log `ok. 1 passed`). Companion C14 interrompu en phase `c14-local-capture-failure-cancellation` START, sans verdict (ni PASS ni FAIL) ; fixture `recipe-02` preservee non reouverte. Deux natives jamais lancees. Disposition `root-runtime-interruption-disposition.json` SHA `6826c742fe6248d41d10a69d10945fe5fb040e7d6cf5f38b69c379c76ddc7cb5`. Arbre de travail 746/746 identique au gel r202.

Reprise : `D:/Layrs/tmp/provider-c14-local-capture-run-r202-resume1.py r202-resume1`, meme EXE r202 gele, trois recettes restantes (companion, Ordinary edit/capture/restore, restore cancellation) en serie, nouveau parent `layrs-provider-c14-content-r202-resume1`, arret premier echec, SetThreadExecutionState anti-veille. Resultats `c14-content-results-resume1.json`, logs `*.resume1.log`. Apres terminaison seulement : `D:/Layrs/tmp/summarize-c14-local-capture-r202-resume1.py` exit0 ou exit1. Ne pas compiler ni lancer d'autre runtime pendant. Ne pas redemarrer l'hote (~1h30 max).

Prochain inchange : qualification r202, puis Restore local Capture/Inverse (voir investigation ci-dessous).

## Point precedent - r200 termine, quatre natives r202 en cours

r200 termine exit1 : 21/21 recettes non natives PASS. Premiere native plain C14 FAIL101 apres 783.691s sur une assertion de diagnostic : le terminal physique refuse les racines ajoutees par Capture avec `exact protection owner has additional roots`, avant le controle d'inventaire prive attendu par le test. Les deux captures, deduplications et lectures publiques precedentes ont atteint leurs assertions ; aucun PASS de la native complete. Trois natives non executees. Integrite et resultats verifies, SHA resultats `6516853a58d392089dbe193c4b33cc670983ab4555fef499c395a676a8ce5afc`. Fixture echouee preservee sans reouverture.

r201 compilation PASS 60s, 85 warnings. r202 contient la correction de cette assertion et P1 annulation Capture legacy (token lie au head courant, C14 toujours lie au Ready). 746 sources, quatre fichiers changes depuis r200. EXE `04a4d740011593a450e19a0a8aeaf10a1dd41fac1c09287a8ccb1b5a879d9545`, CodeView age190 ; host `07355159850dd65c785fb4db45fd9e69dd6b06e4233ef2ebb5f7f7494200f296` ; release ROOT `450b4cf6e2ffda63b41620f7833cef03bc70f2deb8f132ad49a1aa7f9eb13c95`.

Session55533 ACTIVE : `D:/Layrs/tmp/provider-c14-local-capture-run-r202.py r202`, quatre natives restantes en serie sur nouvelles fixtures C:, arret au premier echec. Premiere native en phase publication au dernier controle. Ne pas compiler ni lancer d'autre runtime. Apres terminaison seulement, executer `D:/Layrs/tmp/summarize-c14-local-capture-r202.py` avec exit0 ou exit1 ; script verifie (sha256, quatre recettes et session55533). Les 21 PASS r200 restent lies a leur artefact, sans revendication sur r202. P1 compile mais qualification native encore attendue.

Tous sous-agents termines. Investigation Restore C14 conservee ci-dessous ; aucun code Restore commence. Prochain : resultat r202, correction d'un echec concret ou qualification du lot, puis Restore local Capture/Inverse avec prefinancement forced-append et conservation des preimages. Objectif global non termine ; aucune pause demandee.

## Point courant - r20021testsPASS, premiere native active ; P1source pret

Session37414 toujours ACTIVE :21/21 non-nativesPASS (3nouvellesSQL,13admission,5Capturelegacy), premiere native plainC14 commencee phase c14-plain-publication. Quatre natives restent ; nepascompiler ni lancerautreruntime. Postrun D:/Layrs/tmp/summarize-c14-local-capture-r200.py avec exit0ou1 seulementapresterminal. Aucun succesnativeC14Capture revendique.

Tous agentsnatifs termines. P1legacyCapturecancel sourceintegree/rootrelue/formatPASS mais NON compilee/NONexecutee. Exactement3sources divergentdugel : retention/cancellation.rs, format/recovery/admitted.rs, session/managed_admission_tests.rs. Token prive DenialReady/LegacyHead ; legacy lieexactcanonicalcurrenthead apres inspect+requiremutable, C14writerresteferme. Native9 fait CASepoch+1 puis failureBeforeCommit/edit/refus/cancel/replay etsuiteCaptureRestore. Patch/hashes D:/Layrs/tmp/legacy-capture-cancel-pending-r200.{json,patch}. Nativechild480inchangé, duréeasurveiller ; Ordinary10nonqualifie. Compiler apresfinr200 ettraiterfailureprioritaire.

InvestigationRestoreC14 SOURCEONLYterminee surgelr200 : prochainepetitetranche RestoreSource::Capture localeC seq>=Ready etInverse local, avecresume/cancelprejournal. Version/preparationrestefermee. Nécessite LocalRestorepurpose, readerauthentifiepending/history/preimages before+after (dirtyBnoncapture doitresterretained), financementforcedappend AVANTreserve/staging/FS (Restore ne dedupjamais), budget/claim/stageRestorebranchesfermees, generationReadyfixe/importimmuable. Fichiers ROOTformat/scopes/restoreengine ; interpreterc14_capture+nouv c14_restore ; recoveryengine/recoveryadmitted/retentioncancel ; nativesplaincompanion.Aucun codeRestore commencé. Objectifnonpause/nontermine ; repriseautomatiquefinruntime.


## Point courant - r200 qualification active apres revue r198

Opus r198 termine : aucun nouveau defaut majeur protocoleC14 ; D1Low erreurapresCOMMIT possible sur contention settlement, documentee sansavalererreurs ; P1Medium legacyCapturecancel preexistant a traiter ; H1durcissement applique(ROOT) finish_budget avantvalidationfinale dansmemetx. r199 compilePASS12.17s85warnings. Gelr200746sources,18delta r194 et2petitsfichiersdelta r198(capture.rs doc etstorage.rs ordre). HostSHA ccc74565d82810122f26ae7fa2b17455a8c1e2baf9cb7160780bba71150b4b2b ; EXEage189 SHA7ae7501862c00fdc0ff5ce88b41175508a5a4c57c9b45dffc2adfe7a09e75580. ReleasetestsROOT a3ea52ced28b2c15e7d39a5d7131c681e57b9358072eda5771b0bc9d598e19cf ; runner D:/Layrs/tmp/provider-c14-local-capture-run-r200.py SHA9e419d7517c7038a0d928d9888d537b8f21f50ce44a6e5f9414eee6e0d2a6dfb.

Session37414 ACTIVE :25recettes serialfreshC stopfirstfailure,2batchSQLPASS au dernierpoint. Ne pas compiler/executerautrependant. Verifierpostrun prepare D:/Layrs/tmp/summarize-c14-local-capture-r200.py ARGexit0ou1, a lancerseulementapresterminal. Aucun gelr196/r198execute.

Worker natif c14_capture_cancellation relance SOURCEONLY surP1legacy : retention/cancellation, recoveryadmitted, sharedcancellation siutile etmanaged_admission_tests. Source live peut divergerdugelruntime ; nepastransfererresultats. Pas de fixtures/gels/logs touchesparworker ; nepascompileravantfinr200. P1 legacyhead doit etre actuel, C14 resteReadyfixe. Suite :resultatr200prioritaire, corrigerfailuresconcrets puisintegrerP1. AutonomieRestoreC14/reexport/produitL0-L9 restants ; objectifnontermine.


## Point courant - corrections Capture r198 compilees, revue annulation a lancer

Revue Opus r196 NON ACCEPTE, jamais execute : open generalwriter refusait C14 ; terminalnegative budgetcommitted ; borne apresreserve ; coutquadratique ; cancellation changedimage absente. ROOT corrige openpurpose RO seulementC14/legacy preserve, settlement succes et terminalnegative. Workers integres : preflight exact avantreserve, cap64MiB/100k avec dedup, batch snapshot1x/SQLcursor1x, orphelins seulementmainWRITE+budgetactive ; annulationCapture tokenlieadmission/identity/operation, protocolefamille exact, nobudgetreplayRO. Natives ajoutent BeforeCommit changeimage cancel/retry et AfterCommit boundcancel/replay. Followup D:/Layrs/tmp/c14-local-capture-root-followup-r196.md. F7 reste internalAccessconvention, testsOS/crash/Tree/matricecomplete absents.

r197 compileFAIL1 importchildprive ; ROOT utilise wrapperowned_test existant et rend negatifgeneration discriminant. r197b PASS55.20s85warnings. Gel r198746sources,18delta r194 et12delta r196 ; EXEage188 SHAfc666cf3f51f15cab6f7f8fe8ba7181c1d7c45f073e9b4e25208c47811b2eaff ; hostSHAb5f6dd0cef567a9d58027f86854492a622325586e0f31aa5686d0ea437742782. 25candidates=21non-native+4natives ; aucune execution/release. Runner D:/Layrs/tmp/provider-c14-local-capture-run-r198.py SHA63ed9b30700e040551be22d7173cf07aabb58c3ee1e5faec3e49de4cf7689d10, lieignoreinventory+patches maintenant. DeuxnativesC14incluentancienhelperterminal avantCapture, legacyCaptureRestore et cancellationRestore inclus.

Brief D:/Layrs/tmp/c14-local-capture-cancellation-review-brief-r198.md : revueOpus focalisee nouveauprotocoleannulation etprefunding/batch. Tous workers finis, aucunruntime/buildactif. AttendreOpus siquota, aucunremplacement. Prochain integrerrevue, dispositionROOT puis serialfreshC. Goal/programme non termine. r19443recettesagregees reste evidence precedente.


## Point courant - r196 compile/gele, revue independante a lancer

r195 compile FAIL5 imports test absents ; ROOT corrige imports format/SessionMode et largeur64 des empreintes retention dans lecteur neuf (content IDs71 inchanges). r195b compile PASS57.02s86warnings. Aucun test execute. Gel r196745sources,12fichiersdelta r194, EXEage187 SHA3117713d98744685eb06c45acf819c9bccce10cc450e60370b364db6ac2e6d75. HostSHA837130a3462697155cfb5914f4a3b4bd883ee342d2ef7ca48294a5ac7f438ba7. 21recettes candidates,18 SQL/Capture ordinaires et3natives ; runner prepare sans release donc inexecutable.

Brief D:/Layrs/tmp/c14-local-capture-review-brief-r196.md. Tous workers termines. Aucun build/runtime actif. Revue Opus seulement, attendre quota si limite. Points ouverts explicites : annulation changed-image apres Capture echouee non raccordee (cancel admis reste Restore), TreeReserved/matrice corruption/OSclose/crash non qualifies. Pas de global readiness, pas de pause/fin goal. Prochain revue, corrections bornees puis releaseROOT et execution serie freshC.


## Point courant - Capture locale C14 en integration, pas encore compilee

r194 4/4 PASS et postrun verifies ; bilan precedent 37 DATA/SQL r190 +2 admissions r192 +4 natives r194 conserve. Aucun runtime actif.

Lot suivant borne : Capture locale a generation Ready fixe, acces ferme LocalCapture ; state et lecteurs de Capture utilisent le scope admis authentifie. Generic Exclusive, Restore, preparation, observations et re-export restent refuses. Lecteur generique separe import immuable et suffixe Capture ; terminal physique garde son inventaire exact sans suffixe. Aucun changement de schema. Worker lecteur termine source seulement, worker natives plain/companion en cours. ROOT integre acces, scopes idle et state. Aucune compilation ni execution de ce nouveau lot ; revue Opus independante a lancer sur gel integre, sans substitution modele.


## Point courant - lot admission/reprises termine, autonomie mutable suivante

r194 termine exit0 et postrun `D:/Layrs/tmp/summarize-c14-ack-r194.py` PASS :4/4natives,743sources/EXE/PDB/bindings/logs verifies. ResultatsSHA `64a14eeb7015a2441101d624427ac0c94e09dde8644bcf8987a1240d6ab62805`. Plain4ACK239.327s ; Provider9observationAPI200.282s ; legacyadmissionACK141.723s ; C13Reserved391.631s. Disposition `r194/root-runtime-disposition.json`.

Bilan du lot :37 DATA/SQL PASS r190 +2 admissionsC14 plain/companion PASS r192 +4 natives PASS r194 =43 recettes distinctes qualifiees sur leurs artefacts respectifs, PAS une unique execution sur r194. Regression observations legacy corrigee et exercee sur Provider9/Ordinary10 SQL et API Provider9 ; admissions C14 via vrai adaptateur session, precancel et reprise holder, terminal physique sans B ; quatre phases ACK plain et regression C13Reserved. Les echecs r190/r192 restent conserves et expliques. Aucun runtime/build actif.

Prochaine tranche : autonomie locale mutable C14 apres admission (Capture/Restore), re-export et CLI separes. Investigateur natif `c14_mutable_admission_scope` en lecture seule pour preciser partage inventaire immuable/tailmutable, retention companions/denied, budgets et fichiers/interfaces. Ne pas simplement retirer les gardes read-only/terminal. Admission C13 avec companion mais sans denial reste a distinguer et non implicitement acquise. Limites crash/OSclose/matrice negatives restent distinctes. Programme/goal non termine et non pause ; preference Opus conservee.

## Point courant - admission C14 qualifiee r192, ACK corrige r194 actif

r192 termine exit1 : admissions C14 plain PASS514.812s et companion PASS488.013s, via vrai adaptateur session/precancel/retryholder puis quarantaine B et terminal physique sans B. Stack overflow non reproduite sur ces deux parcours ; lieu du crash r190 demeure inconnu. Integrite743sources/artefacts/bindings/logs verifiee par `D:/Layrs/tmp/record-c14-r192-partial.py`, disposition `r192/root-runtime-partial-disposition.json`, resultatsSHA `58222a95d2eb3aa62fcff6765dfdc407347baef02e526c98399087a178cd6dd2`.

Troisieme native plain ACK FAIL101 apres261.189s : observateur test demandait transaction_id a projection_provider_transitions dont PK est id. Erreur SQL avalee en false, donc aucune injection, puis expected-error assertion. ROOT corrige uniquement deux requetes du test (observer + gap reader) et ajoute phase au diagnostic ; aucune source produit changee. Fixtures precedentes intactes, aucune reouverture/reutilisation. Trois natives suivantes non executees.

r193 compile PASS19.40s86warnings. Gel r194743sources, un seul fichier delta content_plain.rs, EXEage186 SHA `5ae4771b351151d37171a2bcde39ca65789a6ae2fb313f027315511b7352184a`; host `6f6532e7aab2cae0cc4130386158efd80b7323851a03b8f314b6382c8309ed22`. Release tests `e69d3ff81a838166abb546eb91163f55dc470a462345b321d0dec453a009763f`; runner `D:/Layrs/tmp/provider-c14-ack-run-r194.py` SHA `ad9d4f5f7acbc0973c8927155c76265f7b155a25c4ffe6bf0d72ca18a811eea8`, session49702 ACTIVE. Quatre natives restantes serie/freshC/stopfirstfail. Ne pas reexecuter les deux admissions inchangees ni37DATA/SQL sans nouvelle raison ; leurs preuves restent liees r192/r190.

Prochain : finr194, integrite/results et bilan agrege sans pretendre memeEXE. Aucun agent actif, aucun build pendant runner. Preference utilisateur confirmee : attendreOpus en cas de quota, aucun remplacement Astra pour revue sans accord. Autonomie C14 mutable/re-export/callersCLI et programme L0-L9 restent ouverts ; goal ni pause ni termine.

## Point courant - r190 overflow, r192 natives en cours

r190 termine runner exit1 :37 DATA/SQL PASS, premiere native admission plain FAIL101 apres848.442s, enfant0xc00000fd stack overflow. Aucune assertion metier ni phase precise dans le log ; cinq natives non executees. Fixture echouee conservee intacte, jamais reouverte/reutilisee. Integrite743sources/EXE/PDB/bindings verifiee dans `r190/root-runtime-failure-disposition.json` ; resultatsSHA `2f79cd897668d43e17d409f79a0dcac3b26b701f0a69b759fd30c95eee16a30d`.

Investigateur natif en lecture source termine : pas de cycle trouve dans metadata_basis_floor/prefix/historical. ROOT a inspecte unwindPE et appels directs : gros TerminalSnapshot par valeur via deux scopesSQL. Correction bornee : Box construite dans callback interne, propagee jusqu'au holder ; validations/ordre/pins inchanges. Sous-chemin statique inspecte895352->248168octets, open125688->23880 ; pas un maximum global ni preuve du lieu du crash. Ajout PhaseTimer dans nativeplain/admission/terminal. Default stack et delais inchanges. Adaptateur session C14 du worker egalement integre, precancel Validating/0writer puis memeholder retry. Details `D:/Layrs/tmp/c14-admission-stack-followup-r190.md`.

r191 compile PASS. Gel r192743sources,4fichiersdelta r190, EXEage185 SHA `26d27d389a59885bd50740c91439cacff8b90c5b616363e5945299d17a9a5ca0`; host `47f8385751b956ef16ad9558af762a5369bb31b5a1835206bd57388290c1b8dd`. Release tests ROOT `ec43a4335633eedd6c6b760a98ba56122dd18be711a7de37d5f72aabae57d989`. Runner `D:/Layrs/tmp/provider-c14-admission-run-r192.py`, hash `99d86e9427fd87774c775ee76fd074bc0a64d54f02c20a7c3a312afb106549a5`, session85451 ACTIVE. Six natives serie/freshC/stopfirstfail ;37 DATA/SQL r190 conserves comme preuves r190, pas requalifies r192. Premiere nativeplain en cours ; aucun succes C14admission revendique.

Prochain : lire fin/logs phases r192. Si nouvelle overflow, localiser via derniers marqueurs puis changer hypothese ; aucune repetition identique. Si PASS, verifier gel/bindings/logs et poursuivre scope suivant. Aucun agent natif actif, aucun T3review actif ; pas de build tant que runner tourne. r18644/44PASS reste dernier lot integral termine. Goal ni pause ni termine.

## Point courant - r190 en qualification apres revue r187

Revue Opus r187 terminee NOT CLEAN : F1 High accepte (inventaire observations dans lecteur admis legacy). ROOT corrige avant runtime : comptage integral uniquement C14 ; base metadata Option calculee seulement C14 ; obsolete wrapper companion retire ; observer ACK sans panic avant close. Test SQL Provider9 ajoute, meme regression inseree dans Ordinary10, native Provider9 enrichie avec API publique observation/commit/reopen/replay. Trois corrections compilation precedentes conservees. Notes detaillees `D:/Layrs/tmp/c14-admission-root-followup-r187.md`. Rapport Opus original intact ; corrections ROOT auto-revues, aucune seconde revue independante revendiquee.

r189 compile PASS44.84s86warnings. Gel r190 : 743 sources, 28 fichiers delta r186 et exactement10 fichiers de corrections depuis r187, EXEage184. Host SHA256 `23358de3b0f33f1dada4028d223cda48a6da8417488e1130f877a73b8a510e5e`; EXE `2cfbdfa127ea895f57659a1f32ab41dc341f283d4a1d973802e58948edd2424a`. Release tests ROOT `85dd58a4ae940f734e100b74e41bbbf61c0ed8975859e06a41447677aa7edb97`, runner `78b487a08c0b04be2958becd42328dda3bc605c1ddb31f08775104c1100972e5`.

43 tests en serie sur fixtures C: neuves, session91001 ; arret premier echec. 37 DATA/SQL puis 6 natives, admission plain/companion d'abord. Dernier point :37/37 DATA/SQL PASS, premiere native admission plain active ; ne pas compiler tant que runner tourne. Script postrun `D:/Layrs/tmp/summarize-c14-admission-r190.py` prepare NON execute. r188 jamais execute. r18644/44PASS reste dernier lot complet.

Agent natif `c14_session_admission_adapter` termine : seulement session.rs et c14_denial_start_tests/admission.rs. Adaptateur explicite de session C14 et usage dans les natives, plus refusal precancel Validating/zero-writer/retry holder. ROOT a relu/integré : enum ferme, ancien Legacy inchange, lifecycle/owner/prepared/verification conserves jusqu'au retour, quarantaine inchangee. Rustfmt/check PASS ; aucune compilation ni runtime agent. Exactement ces deux fichiers live divergent du gel r190 ; patch et hashes `D:/Layrs/tmp/c14-session-admission-adapter-pending.{json,patch}`. Compiler seulement apres finruntime et traiter les eventuels echecs d'abord. Limites : plans C14 anciens insuffisamment finances refuses sans resizing ; matrice admission ACK/crash et negatives terminal encore ouvertes ; autonomie mutable/re-export/callers CLI et L0-L9 non termines. Goal ni pause ni termine.

## Point courant - r188 compile et fige, revue r187 en cours

r186 est termine et verifie : 44/44 PASS. r187b a echoue sur trois erreurs de compilation ; r187c les corrige dans deux fichiers et compile avec exit0 (88 warnings, 1m28s). Corrections exactes : import BTreeSet, conversion d'erreur par `?`, retour unitaire apres controle de capacite. JSON et patch `D:/Layrs/tmp/c14-admission-compile-corrections-r187c.*` ; aucun changement au gel source r187 examine par Opus.

Gel r188 : `.layrs-local/validation/provider-c13-history-20261102-r188`, 743 sources, 24 fichiers modifies depuis r186. Comparaison avec r187 : exactement les deux fichiers de corrections compilation. Host SHA256 `04dd4cdc9126bb05aa0a56f2689756c84cc2858d4d745e4751d729c2ad082910`, EXE `1dbddc33b343177e1fa3a8d3b4ce28ddf043af2ff27df83639db51ea5a97e973`, CodeView age183. `compile-source-binding.json` lie gel source, corrections et log de build ; cette liaison reste une attestation ROOT.

Selection r188 preparee : 35 DATA/SQL et 5 natives, toutes presentes dans l'inventaire compile. Trois nouvelles natives C14 (reprises plain, admission plain, admission companion), regression admission legacy et C13 Reserved/reouverture. Les deux natives d'admission incluent le parcours de publication complet. Candidate SHA256 `30e52b21c830875dc37ced59f22f368c785cd6f2ee72020249def81711d2abf3`. Runner `D:/Layrs/tmp/provider-c14-admission-run-r188.py`, SHA256 `ad867d2d13f04dff4e5bfb65767c8db6c307430fc2b6f26b5d241a6553741873`.

Aucune release r188, aucun test r188 execute. La revue Opus source r187 est toujours en cours selon le dernier retour ; ne pas la poller. Sa completion reveillera le thread. Prochaine action : traiter ses constats, lier explicitement les trois corrections compilation, puis release ROOT et execution serie sur nouvelles fixtures. Aucun build/runtime actif. Admission C14 runtime, callers produit et suite L0-L9 restent non termines. Goal ni pause ni termine.

## Point courant - r186 44/44 PASS, compilation admission r187b

Runner r186 termine exit0. Postrun verifier D:/Layrs/tmp/summarize-c14-reopen-parity-r186.py PASS: 740sources, EXE/PDB, bindingsreview/release/runner et logs exacts. Resultats SHA256 0b0ebfbb37fafb6ea103348934dd461441f7b070fd02e2a00244d27e3c1e9bc0. C14plain489.487s, companion497.274s, C13ACK835.394s, C13Reserved+reouverture sanssource753.22s. Aucun resultat transfere au nouveau lot.
Compilation cargo test -p layrs-store-local --lib --no-run r187b lancee apres finruntime, log D:/Layrs/tmp/c14-admission-build-r187b.log, session77108. Revue Opus source r187 task ca54c4ac-02a6-4927-b26f-6a7d0c25a8e5 en cours selon dernier outil; ne pas poll, finreveillera thread. Correctifs compilation eventuels a enregistrer separ?ment du gel source743/delta24.


## Point courant - r186 runtime actif, admission C14 r187 source prebuild

Runner r186 session28120 actif: 42/44 PASS au dernier point, toutes DATA/SQL, deux natives C14 publication sans/avec companion PASS (489.487s/497.274s); les deux C13 restent a finir. Ne pas compiler en parallele. Aucun r186 runtime-disposition final tant que runner non termine. Script postrun D:/Layrs/tmp/summarize-c14-reopen-parity-r186.py prepare mais NON execute.
Nouveau lot source r187 integre, NON compile/non execute: admission explicite C14 Provider8->9, lecteur frozen documents/companions reel, metadatafloor transporte, admission peak avant Planned, terminal physique source-independent readonly, gardes legacy/mutation/export fermees. 24 fichiers delta / 743 sources dans .layrs-local/validation/provider-c14-admission-source-20261102-r187. Disposition SHA545ff97e564f978a98b65c76554c15c038765ed2c1bf30c861c0014874adcc9b.
Trois nouvelles natives: matrice4ACK plain, admission plain et companion avec quarantaine seule DB B puis fresh C9 terminal; testbudget DATA ajoute. Tous agents natifs termines. Revue Opus SOURCE ONLY prebuild a lancer via brief D:/Layrs/tmp/c14-admission-terminal-review-brief-r187.md, rapportsuniques c14-admission-terminal-critical-review-r187-opus.json/.md. Ne pas transferer ancienEXE/qualification a ce lot. Prochain: resultat revue, finr186/postcheck, compile r187b puis freezeartifacts r188+selection exacte et releaseROOT apres correction. Pas de commit/push ni arretgoal.


## Point courant - revue r182 terminee, corrections de parite r186

Opus a repris et termine sa revue; aucun changement de modele. Les deux exemples de cibles immuables changees sont refuses par le normaliseur. Deux variantes admissibles traitees avant planning: manifeste alternatif absent des inventaires autonomes, et ancien chemin Reserved devenu lisible sans fichier dans image B. ROOT corrige les deux fichiers transition_reconcile/companions.rs et tests/companions.rs; trois nouvelles recettes DATA, aucun affaiblissement lecteur. Detail: D:/Layrs/tmp/c14-plain-denial-root-followup-r182.md. Rapports Opus conserves intacts.
r183 compile PASS49.44s, r185 PASS34.78s86warnings. r182/r184 non executes, aucun resultat transfere. Qualification r186 en preparation sur gel neuf; r18011/11PASS reste derniere preuve du parcours companion. Admission C14, callers publics et suite L0-L9 restent a faire. Goal non pause ni termine.


## Point courant - r182 revue Opus interrompue par quota

Opus task51a8f906-b67b-4631-92ca-3e656ee8e7bb failed sur quota5h; reset annonce20:50Paris2026-11-02. Revue partiellement lue, AUCUN rapport final, aucune release r182, aucun test r182 lance. Pas de remplacement modele sans accord.
Question utilisateur async en attente : autoriser GPT-6-Astra pour finir revue privee r182 OU attendreOpus. TacheOpus originale conservee pour repriseautomatique possible; si remplacee, la cancel explicitement et sortiesuniques obligatoires.
ROOT a suivi une piste unionlive/autonome : changement de cibleFileId deja refuse parnormalizer; representationManifestId alternative pour memeFileId demeure hypothese a examiner. Notes `D:/Layrs/tmp/c14-plain-denial-root-followup-r182.md`. Aucun produit modifie/build/SQL/natif pendantcefollowup. Sources/gels restent r182compile740/20/21. r18011/11PASS conserve. Pas de pause ni cloturegoal.

## Point courant - r182 compile, revue C14 sans companion a lancer

r18011/11PASS conserve (resultats ad2d9dcd0b873fddf50eb60d9f7a1323af2bc9d6a53644096605569ad31784c9). Aucun runtime actuel.
r181 FAIL101 unique E0308 return decode driverDenial, corrige Ok(...?). r181b compilePASS64s86warnings Cargo0 session28309 terminee.
Gel r182740sources/20fichiersdelta dont3nouveaux; EXEage180. Host `c827d3a1655b8e8320d28c8bc54881156b33a5fe91aee265cb5a4934547b7ed4`; EXE `2428219ad01ded171c3979ef0cc884f2e055588d8a84c672d0526452d2e82c2d`.
Route closed commit_denial+DenialPins sans s/header, ReadyLive/RetainedDenial explicite, vrais snapshots/selection/corpsC, imageB et fundingd. Metadata prospectif inclut publicationB sans s. Native ContentPlain complete puis reouverture physique apres liberationSession. T2 couts divergents ajoutes au natifCompanion existant. Tous workers SOURCE STABLE et termines.
21recettes preparees NON executees, aucune release r182. Candidate `80e36eed58ae3f9d85630986d145e7836eee8224c92dcae06b89ceabd63924f2`; runner `f3f3545310c10b9e5493b9a5c8dfdd35f58998aa45c0e475b53524e64433abfc`.
Brief D:/Layrs/tmp/c14-plain-denial-review-brief-r182.md; sorties uniques c14-plain-denial-critical-review-r182-opus.json/.md. Prochaine action revueOpus, traiter defauts concrets sans nouveaux cyclesdelegues pour petites corrections, puis releaseROOT et qualification serie sur fixturesfraiches.
Limites : absence callerproduction, admissionC14 reste refusee, ACKmatrixv2/freshowner incomplets, aucun L0-L9 completion. Pas de pause/fin du goal demandee ni appliquee.

## Point courant — r178 terminé, préfinancement complet en intégration

**r178 : 15/15 PASS**, session4327 terminée exit0. Douze DATA/SQL, native C14
complète313,283s, C13 reprise ACK688,396s et unchanged Reserved403,942s.
Intégrité postrun vérifiée :735sources gelées exactes, EXE/PDB, liens de release
et logs. Résultats SHA256
`fd50e20ddff8498f5f77bcbaf9e881b7467f44fd0d8eb5baa64b1db83560805b`.
Disposition `r178/root-runtime-disposition.json`. Le parcours C14 qualifié est
celui **avec companion réel** ; pas de route C14 sans companion, admission
réussie ou autonomie physique C14 revendiquée. r176 FAIL reste conservé.

Lot suivant NON compilé : ProspectiveCompanionCost DATA dans source_companion
(worker c14_source_receipt_v2), borne du reçu64KiB avant B et du financement s
sans faux Marker/ExactBinding/Ready. ROOT raccorde le coût dans denial_plan,
start/staging, helper shared source_denial, opener content et native. Préflight
U+R+absent_d+q+s ; ancien None refuse un plan avec companion sans coût fermé.
Native ajoutée au parcours existant : seuil moins1 refuse avant B, seuil exact,
ACK famille puis reprise sans double d, Planned durable sous quota réduit sans
writer, actual publication s borné par coût retenu initialement. Le runner r178
était gelé et n’a exécuté aucun de ces nouveaux edits. Aucun build/runtime actif.
Attendre SOURCE STABLE du worker avant compilation r179 ; nouvelle revue
critique après intégration du calcul prospectif, puis qualification ciblée.

## Point courant — correction Ready r178 en qualification

**Mise à jour r178 : native C14 complète PASS313,283s.** Les douze régressions
courtes passent aussi. Ready privé/famille C, B Published, replay0writer,
corps/reçus v2 exacts, corruptions corps absente/substituée/extra refusées sous
rollback, admission SQL sans mutation et admission physique Validating0writer
sont réellement exercés. Session4327 poursuit les deux C13 ; aucun résultat
C13 r178 revendiqué avant leur fin. L’ancienne mention « pas encore C14
qualifié » ci-dessous est remplacée par cette preuve bornée, sans admission
réussie/autonomie physique C14 ni clôture globale.

r176 terminé exit1 : **27 PASS**, native C14 FAIL101 après218,034s à
`Ready data snapshot or capture commitments differ`, deux C13 non lancées.
Échec et fixture conservés, jamais rouverts. Diagnostic ROOT : C14 engage
la sélection fraîche, alors que Ready DATA et suffixe comparent encore celle
de la source ; garde denied inconditionnelle également présente.
Trois fichiers corrigés : Ready DATA utilise la sélection du corps scellé,
suffixe recalcule le digest sélection/génération/snapshot frais en v2, mode
companion v2 reconstruit le corps complet depuis la vraie observation et exige
sa liaison exacte. Le corps SQL et son namespace restent requis ; v1 inchangé.

r177 erreur de compilation sur `.record()` d’un record déjà décodé, corrigée.
r177b PASS20,29s Cargo0,85warnings. Gel r178735sources, trois fichiers changés,
âge178, host `4391b20947a49cacde54002a8df35c09b9d6cc3eb18ce0becddffd6af150f58f`.
EXE `992e48c43a9eb5a3415b65aed9be162ab43af31f8bbc118a210eb18bcad4845b`.
ROOT review `tmp/c14-ready-selection-root-review-r178.md`; release
`33af6970756b94df5475cbc30e652a6cc4f7674656d640fad2e877adcbd606dd`.
**Session4327 en cours** : douze régressions Ready/prefix/DATA PASS puis native
C14 en cours ; deux natives C13 à suivre. Aucun autre build/SQL/natif parallèle.
Runner `tmp/provider-c14-ready-selection-run-r178.py`, résultats dans
`r178/c14-content-results.json`. Pas encore de C14 Ready/Published qualifié.

## Point courant — revue r174 corrigée, qualification r176 en cours

Opus r174 terminé : F1 moyen (admission v2 refusée après ouverture writer),
F2 faible (corps lu avant rôle/état/FORMAT), lacunes de tests séparées. Rapport
`tmp/c14-content-coverage-critical-review-r174-opus.json`, SHA256
`8679f703967acec32f263c933cd357116c32fad1bde78780225d54877bbe4604`.
ROOT a corrigé directement ces petits findings, sans nouvelle délégation.
F1 : garde au début du commit physique, dans clean_admission et admit_ready
avant toute mutation/budget ; F2 : liens légers vérifiés avant corps.
Native réelle étendue : admission SQL et physique refusée sans mutation,
step Validating/0writer ; trois corruptions du corps Ready C sous savepoints
annulés, erreurs exactes et aucune réparation, full reader après rollback.

Compilation r175 PASS32,85s Cargo0,85warnings, session17448 terminée.
Gel r176 : 735 sources, cinq fichiers changés depuis r174, âge177.
Host `072d0b379bd5a3893887bcda468b12a68afb98cf17bcd29bfc814cf4d927265e`.
EXE `9f925945c5bc2ce1059d81f1e08999e78395843e460421c3d837fbb181ef6fdb`.
ROOT delta review `tmp/c14-content-guards-root-review-r176.md` ; release
`a5eb9ca0afd132e2a5ff7827a6eaf003f55766ea2f430541c7c615d6a7371f80`.
Runner `tmp/provider-c14-content-guards-run-r176.py`, **session61561 en cours** :
26 DATA/SQL PASS puis native Staging en cours au dernier point. 30 recettes
au total ; suivent C14 complet, C13 reprise ACK et C13 unchanged Reserved.
Pas de native C14 Ready/Publiée ni refus d’admission qualifié pour l’instant.
Ne pas exécuter un autre build/SQL/natif pendant cette session. Logs/results
dans `r176/c14-content-results.json`. Stop premier échec ; pas de fixture reuse.

## Point courant — 2 novembre, contenu C14 r174

Ce bloc remplace les états « en cours/non compilé » plus bas. **Compilation
r173b PASS38,05s**, Cargo0,85warnings, session42875 terminée. Gel r174 :
735 sources, 13 fichiers changés depuis r172, RSDS âge176. Host SHA256
`909b900fa9b069c6c4dbdfe708abaa36d3cfcf4f9bdf8118f5e69b55181be612`, EXE
`b1d8bb12b7d5b2d55bbe8b38e543ec68b9d4c22cfa312b9547771635dad90891`.
**Aucun test r174 exécuté, aucune release d’exécution encore émise.**

Intégration : opener et wrapper content C14 fermés, vrai anchor Session acquis
avant Planned, corps scellé revalidé contre SQL/observation, d conservé dans
le contexte famille complet, reçu v2 source et Ready, lectures protection B
strictes, inventaire et coût du corps courant dans Ready8. Les lecteurs
Admitted/Ordinary denial restent explicitement refusés. Nouvelle native réelle
begin→allocation→copie→Staging→C Ready privé/famille→B Published, puis replay
0writer ; README modifié devient Reserved dans cette seule observation.
Pas de succès runtime revendiqué sur cette nouvelle chaîne.

Brief : `tmp/c14-content-coverage-review-brief-r174.md`. Revue indépendante à
lancer sur ce gel avec sorties uniques `tmp/c14-content-coverage-critical-review-r174-opus.{json,md}`.
15 recettes candidates (12 DATA/SQL, Stage DATA receipt, nouveau parcours C14,
régression C13), runner préparé `tmp/provider-c14-content-coverage-run-r174.py`.
Après revue : ROOT corrige les findings concrets, recompile/gel nouveau si
nécessaire, puis release et qualification exacte sérialisée en fixtures fraîches.
Tous sous-agents natifs ont terminé ; aucun build/runtime actif.

Limites inchangées : d+q initial n’inclut pas s ; matrice ACK v2/corruption
Ready encore absente ; admission/autonomie physique/re-export, caller produit,
lease serveur et reste L0–L9 non terminés. r170 13/13 et r172 4/4 PASS restent
des preuves séparées. `get_goal` affiche toujours blocked ; ROOT n’a pas changé
ce statut et l’API disponible ne permet pas resume. Travail autorisé poursuivi.

## Point courant — 2 novembre, gel Staging r170

**r172 terminé : 4/4 PASS, session85322 exit0**. SQL CAS et agrégat PASS,
reprise BStaging/CCopied sous quota futur réduit PASS101,897s (un seul writer C),
refus de capacité avant B toujours PASS99,416s. Disposition dans
`r172/root-runtime-disposition.json`, release
`e635610619ef44629fec6e48bede9044f9d87b14b0d6e264077552497f5fce4b`.
Deux revues Opus limitées avaient repris automatiquement et écrit le même
rapport r170. Résolu sans modification de sources : première revue récupérée
EXACTEMENT depuis file_change T3, hash de925b4b…a093 rétabli au chemin original
et conservé sous `tmp/c14-staging-critical-review-r170-first.json`. Deuxième
conservée sous `...-second.json` hash f7142a2a…4c9a. Aucun HIGH/MEDIUM dans les
deux. Nouveaux résiduels du second : corruption du corps après Staging non
exercée et ancienne sélection ACK/recovery non transférée à r170 (explicitement).
À l'avenir, annuler même les anciennes tâches limitées avant remplacement,
et utiliser des sorties uniques pour chaque revue.

Travail suivant en cours, NON compilé/exécuté : source_companion reçoit une
variante fermée derive_with_coverage/planned_receipt_with_coverage et garde
legacy explicite ; Ready DATA reçoit new_companion_with_coverage. Le raccordement
content doit conserver vrai ProviderTransitionSource acquis AVANT Planned :
il prouve connexion/lifecycle et historique complet, non remplaçable par le
plan/union seuls. Caller peut le retenir séparément, puis le fournir au nouvel
opener C14 après libération de PendingDenialStart et conservation de l'image.
Ne pas reconstruire cet anchor après Planned ni supprimer son contrôle.

**Dernier état : r170 runner terminé exit0, 13/13 PASS**, y compris document
substitué et deux legacy. Résultats SHA256
`78a0d8eb0e324591ecf9f1a1c8c3f9e1486de05c381fbf6b2fbbc1545b053666` ; disposition
`r170/root-runtime-disposition.json`. **r171 compile PASS29,89s**, Cargo exit0,
85 warnings, log `tmp/c14-staging-gap-build-r171.log`. Aucun runtime/build actif.
**Gel r172** : 734 sources, 3 fichiers changés depuis r170, PDB age174.
Host `3d32dbbc3b174c2ff2f8fe2958c8484a228627ce3111cb01e7c78c13139fd1b7`, EXE
`02241d6c24fb24258011b7ff1d27539ffc8b69ca0d8332ae5290f933d3d8bd13`.
LOW1/2 corrigés en source : après BStaging durable, reprendre le seul CAS C
sans réadmettre q futur ; native ACK B sous quota réduit (un writer C, famille
et B inchangés) ajoutée. LOW3 : deux négatifs SQL du CAS et de ses bornes ajoutés.
Quatre recettes ciblées compilées, non exécutées. Brief
`tmp/c14-staging-gap-review-brief-r172.md`, runner préparé
`tmp/provider-c14-staging-gap-run-r172.py` (SHA256
`9ea9f1ec8a63800577b89733a955b5e26aea05667cf1afe459a41fe7ce55c537`).
Vérification ROOT des trois petites corrections, conformément à la règle T3
« Fix small review findings yourself; start no delegate for them ». Pas de
nouvelle revue indépendante revendiquée : celle d'Opus reste liée à r170.
Qualification ciblée des quatre recettes après décision ROOT, sans réexécution
générale des 13 anciens PASS. LOW4 reste ouvert pour content C14.

**Mise à jour après reprise automatique Opus : revue r170 terminée**, aucun
HIGH/MEDIUM. Quatre LOW : capacité future réimposée dans le gap BStaging/CCopied,
absence de son test sous quota réduit, négatifs preflight CAS absents, garde
legacy v2 seulement implicite (plausible, audit non exhaustif).
Rapport SHA256 `de925b4bef316a8d02f11c0cc2a51e87d3984e2fc88300d46f8aef2ba6f3a093`.
ROOT a renforcé le runner (host/PDB/review/13 candidats liés avant création de
fixtures), puis émis la release bornée
`8a311822cb0ca24267a960d5a85e1c75178fcddc218940e823c9b159c93c2998`.
**Runner r170 session89641 en cours**, aucune acceptation globale. Les huit
premières recettes sont PASS, notamment vrai Staging, ACK B/C, capacité avant B.
Attendre le résultat final et arrêter au premier échec comme prévu.

Modifications live post-gel NON compilées/exécutées : suppression du contrôle
prospectif q uniquement après BStaging durable, reprise native après ACK B sous
quota réduit, deux SQL négatifs du preflight CAS/agrégat. Le test d'agrégat utilise
du padding SQL explicitement synthétique pour le seul helper arithmétique,
jamais comme preuve source complète/native. Fixtures rollback et source relue.
Ne pas transférer les PASS r170 à ces changements. Le finding legacy plausible
reste ouvert ; il sera traité dans le raccordement C14 fermé avec gardes C13.
Tous agents natifs terminés. Notes de raccordement content dans
`tmp/c14-after-staging-integration-notes-r170.md` enrichies : d traverse déjà
le contexte famille exact, aucun double vecteur/double débit à introduire.

Cette section prévaut sur les notes historiques ci-dessous. **r168 est qualifié
sur ses 17 recettes : 17 PASS**, runner session78936 terminé exit0 après revue
Opus. Résultats `provider-c13-history-20261102-r168/c14-sql-results.json`, SHA256
`1e1d9c03043c6176e5216b83d0a3fde6037601b5ed8c8b93fe0d9065d38b4cc3`.
Revue `tmp/c14-capacity-critical-review-r168.json` : aucun HIGH/MEDIUM, 4 LOW.
Release ROOT `b187cbb02eef2ed3e851dd3b297cf19975922672c4ba02304d5943cd664c8f3a`.
Les deux recettes différées de r166 (ambigu/history) passent en r168.

Le delta suivant est compilé mais **pas exécuté**, et doit être revu avant
release runtime :

- `stage_denial` réel : SourceCopying/CCopied vers dual Staging, puis reprises
  BStaging/CCopied et dual-Staging sans writers inutiles. Source actuelle,
  corps, union, financement d et famille complète relus à chaque frontière.
  Refus C ahead et corps absent/substitué ; aucune réparation. Snapshot source
  retenu jusqu'au COMMIT/close/readback C. Aucun C reserve/companion/Ready.
- Préflight aggregate du CAS source avant B et sous writer ; contrôle d+q
  prospectif avant nouvelles écritures, omis pour dual-Staging déjà durable.
- R168-3 : Session du guard et Session d'allocation liées au vrai root source
  par registration encodée/catalog identity et revalidation, avant effets.
- Natives R1 enrichies : Planned durable sous quota réduit, parent remplacé
  sous le même chemin, guard/Session étrangers. SQL capacité : id/phase,
  autocommit, doublon opération et installations négatifs supplémentaires.
- Sept nouvelles natives Stage : succès, ACK B, ACK C, quota, C ahead,
  corps absent, corps substitué. Compteurs writers et full readbacks.

r169 a échoué sur la visibilité du nouveau helper ; corrigée. **r169b compile
PASS38,94s, Cargo exit0, 85 warnings**, log `tmp/c14-staging-build-r169b.log`.
**r170 : 734 sources, 11 chemins depuis r168, RSDS age173.** Host SHA256
`afbca5ade059f5d8d42d65de97aa636f3b6da866421efa5ca70380c977c74393` ; EXE
`2dd06a74d7d747c6772f670e496bf47a94e03db631b4755d50e646c82c0bbd36`.
Treize recettes ciblées dans `c14-staging-candidate-recipes.json` : sept Stage,
R1, SQL capacité, transition SQL, current coverage et deux legacy Staging.
Brief `tmp/c14-staging-review-brief-r170.md` ; runner préparé mais non exécuté
`tmp/provider-c14-staging-run-r170.py`. Pas de release runtime r170 à cette note.
Tous les agents natifs ont terminé ; aucun build/test actif.

La revue Opus r170 a été lancée puis a échoué immédiatement :
`Claude API rate limit reached. Try again later.` Aucun verdict reçu, aucun
résultat inventé ni release runtime. Tâche `713bb258-0c31-421a-a425-c62626ae8ef3`.
Le gel et le brief sont conservés pour une nouvelle revue quand le fournisseur
est disponible ; aucun autre modèle substitué sans instruction.

Après l'heure annoncée de reset (15h50 Paris), nouvelle tâche de revue
`61b08307-74ab-4961-864a-242c30def1f7` : même échec fournisseur immédiat.
Pas de nouvelle tentative en boucle. Revue r170 toujours absente. Vérification
ROOT finale : 734 sources vivantes et gelées concordantes ; runner syntaxe OK,
SHA256 `8b70700aebe19a7a6daa905196d960c980417301ec1112e538bdfd867f1e18ba`.
Préparation de la suite, lecture seule :
`D:/Layrs/tmp/c14-after-staging-integration-notes-r170.md` (receipt v2, d+q+s,
constructeur C14 fermé, gardes legacy maintenues). Aucun test r170 exécuté.

Suite : revue intégrée indépendante, correction de findings s'il y en a,
release ROOT puis qualification ciblée sur racines C fraîches, arrêt au premier
échec. Ensuite raccorder C14 content/protection/Ready/admission/autonomie et
le reste L0–L9. Le sizing companion B `s` reste distinct de d+q, sans faux proof.
Pas de qualification panne OS close/powerloss ni autorité serveur continue.
Pas de commit/push ; stash original conservé. Le suivi `get_goal` est apparu
`blocked` à la reprise du contexte ; aucun appel pause/complete/blocked n'a été
fait par ce tour et le travail autorisé continue (l'API ne permet pas resume).

## Travail parallèle après r166 — capacité prospective et refus recovery

La revue Opus du gel r166 est terminée, distincte des modifications suivantes du
worktree. Son runner (`tmp/provider-c14-copy-run-r166.py`, SHA256
`28b53a6914d12f8463514fd58065543a287fa0fcb5f5745df70209779876adc1`), sans release
à sa préparation, a reçu la décision ROOT après revue. Rapport JSON SHA256
`95f596754e85e2868d3f20fe8eb85341abfe780d2c0300e3479a7b7bd692c6c7`, release
`d742ec65c3b627af72884fe05496e4f788bc6d9d639626e31c8a178ac85b95ac`.
**Runner session14350 terminé exit0 : 36/36 recettes retenues PASS.** Preuve
`provider-c13-history-20261102-r166/c14-sql-results.json`. Deux explicitement
différées (négatif alias avec unwrap confirmé, histoire dont taille minimale non
garantie) : aucune des deux n'est PASS. Qualifie begin complet, allocation/FORMAT,
copie C/replay, altération Planned/image/preuve, legacy, ACK source/famille/C,
orphan release, conservation du Planned après perte owner et SQL sélectionné.
Ces résultats ne qualifient pas les nouveaux changements worktree.

- R1 partiel implémenté : helper SQL read-only lié au plan complet ; avec usage
  contenu U, réservations R, document B d et quota C q, vérifier U+R+d+q si B
  absent, U+R+q si B exact présent. Enveloppe physique R+d+q ou R+q sur famille
  et vrai parent C. `PendingDenialStart::prepare` retient désormais un véritable
  ExportParentGuard fourni par le caller, lié au chemin/identité du plan.
  Contrôles dans prepare, avant reçu dans begin, puis sous BEGIN IMMEDIATE avant
  B. `require_retained`, recovery et reconnaissance du Planned durable restent
  sans cette exigence prospective. Les futurs writers doivent recontrôler.
- Deux SQL ciblés ajoutés : limite exacte/-1 avec CAS+sweep+journal+réservation
  étrangère et B compté une seule fois ; plan changé et collision refusés sans
  effet. Aucun n'est exécuté à cette note.
- F2-R157-1 : quatre natives du vrai recovery opener ajoutées. Orphelin sans
  reçu, reçu en conflit avec intent, corps sans intent, seconde opération B
  étrangère à la release. Départ par vrais gaps begin/ACK ; corruptions de
  fixture explicitement nommées. Zéro writer et témoins source/catalog/reçu
  inchangés exigés. Corps sans intent qualifie le full inventory en amont,
  pas directement le EXISTS interne. Aucun runtime encore.
- r167 : E0583 chemin du nouveau sous-module test, corrigé par #[path].
  **r167b compile PASS49,29s, Cargo exit0, 85 warnings**. Log
  `D:/Layrs/tmp/c14-capacity-recovery-build-r167b.log`. Aucun gel/exécution de
  ce binaire. La native supplémentaire d+q est maintenant écrite et formatée :
  `c14_denial_start_rechecks_source_and_future_destination_capacity_before_funding`.
  Quota −1 avant prepare puis après prepare, seuil exact, vrai ACK famille puis
  retry B déjà présent (sans double compte), Planned replay, vrai mauvais guard
  parent. APIs usage/set_quota réelles ; aucun claim de panne disque physique.
- Trois corrections r166 ajoutées APRÈS r167b : propagation du candidat strict
  par and_then(valid), historique Cancelled avec locator DATA borné assez long
  pour dépasser les bytes avant MAX_ROWS, scopes copie/readback exigeant famille
  Initializing (Absent permis seulement avant registration, jamais après succès).
  Caller test passé à registering=false ; aucun changement au gel r166.
  **r167c compile PASS36,95s, Cargo exit0, 85 warnings**, après runner terminal.
  **Gel r168 créé : 732 sources, neuf chemins depuis r166, PDB age172.** Host
  `3fdd641778f7a4a37f65dff1c2f1c12224332fc20f68de5c18ecca0e9647b968`, EXE
  `c90ff68514f27cdd4be02b16daaeb5eba8f37856e84ae542396517a07169a1d8`.
  17 recettes ciblées compilées, aucune exécutée. Brief
  `D:/Layrs/tmp/c14-capacity-review-brief-r168.md`. Revue ciblée indépendante
  capacité/recovery/corrections avant décision ROOT d'exécution. Aucun runtime
  ni build actif à cette note. Staging est la prochaine intégration produit ;
  la condition préalable de vrais succès begin/allocation/copie est satisfaite
  par r166, sans qualification de la nouvelle capacité d+q.

Limite R1 explicite : la réservation companion B `s` est une troisième charge.
Le constructeur actuel exige vrai Staging et identité C future ; aucun faux
proof n'est construit pour calculer `s`. Il faut une primitive de sizing DATA
distincte au raccordement C14 content. Le présent contrôle d+q ne qualifie pas
la capacité complète du parcours companion, ni une garantie entre verrous.
La garde parent ne remplace pas les exclusions famille/control de l'allocation.

## Point courant — 2 novembre, gel r166 (prévaut sur r162)

**Goal actif ; plan global L0–L9 incomplet.** Aucun runtime en cours à cette
note. Aucun commit/push ; stash original préservé.

- Revue r162 terminée : aucun nouveau produit HIGH/MEDIUM, couverture négative
  allocation insuffisante. Qualification r162 : **DescriptorOnly orphan recovery
  PASS18,892s**, puis allocation **FAIL101 avant begin** (6,512s). La fixture
  CurrentAlias avait deux descriptors originaux de même FileId et ManifestIds
  différents ; le produit refuse exactement cette ambiguïté. Reste non exécuté.
- Toutes les fixtures positives sont désormais DescriptorOnly : README
  fixed1MiB dès bootstrap, immuable distinct, préparation publique. Alias ambigu
  devient un négatif explicite ; Nonresident ne manque que la préimage exacte.
  Aucun relâchement du contrat de pin ou de mapping produit.
- C14 `register_and_copy_denial` ajouté : scope fermé du même owner, source SQL
  retenue, vrai B/current body/plan/image, famille Initializing, C EmptyIngress
  puis CopiedPrefix avec corps exact. Replays family/C ACK et sans writer.
  Legacy garde ses refus ; lectures directes du module devenues strictes.
- Corrections r162 : preuve copiée explicitement égale à l'allocation réelle ;
  owner utilisable seulement après begin acquitté et reçu nettoyé. Préflight
  réserve MAX_BYTES de croissance de l'intent avant B dans la limite 16MiB.
- Cinq nouveaux négatifs : owner non soldé, Planned modifié, routes legacy,
  Copying proof attendu modifié, image native modifiée. Trois natives copie
  et régressions legacy retenues. C déjà enregistré reste non couvert.
- Compilation r163 PASS1m02 puis r165 terminée en 1m03 avec EXE et 85 warnings.
  Wrapper PowerShell r165 exit1 sur NativeCommandError stderr Cargo ; aucun
  diagnostic de compilation error dans le log. r164 intermédiaire non exécuté.
  **r166 : 730 sources, six chemins depuis r162, PDB age170**. Host
  `24cf02dc8aa106ff624ea0dc7d4bdd613130601765e579e13751dc91712972be`, EXE
  `44e93522a058bd562ba9d9d9c94186e1aa0376de098d6aa4b27a29fd97aa3231`.
  38 candidats compilés, aucun exécuté ; doublon orphan DescriptorOnly omis.
  Brief : `D:/Layrs/tmp/c14-copy-review-brief-r166.md`. Revue indépendante
  et décision ROOT requises avant le runner, pas de release à cette note.

Prochaine validation : vrai begin réussi DescriptorOnly, allocation puis copie,
avant d'empiler Staging. Arrêt au premier échec, aucun fixture rouvert/réutilisé.
Preuves anciennes distinctes : r157 16 DATA/FS/SQL PASS ; r154 cinq DATA PASS.

Suite produit identifiée : `stage_denial` avec matrice réelle BCopying/CCopied,
BStaging/CCopied, BStaging/CStaging ; BCopying/CStaging doit refuser. Conserver
les gardes legacy, snapshot source jusqu'au close C et corps réellement relus.
Ne pas élargir SourceCurrentDeniedCoverage (Copying seulement). Préflight futur
quota C et capacité physique avant effets à ajouter ; recontrôle au writer.
Content C14 exige un constructeur coverage-aware, pas la suppression des gardes
C13 : deux obligations B distinctes, document et preservation companions.

Restent F2 négatifs recovery, share-all du reçu, probes FS, OS-close/crash/power-loss,
F7 GC/rétention, historique Published/Reserved, Stage/Ready/admission/autonomie,
caller client/autorité distante réelle et reste du plan global. Pas de clôture
de C14 ni du goal.

## Point courant — 2 novembre, gel r162 (prévaut sur r157)

**Goal actif, aucune étape globale déclarée terminée.** Aucun runtime actif
à cette note ; aucun commit/push, stash original préservé.

- C14 allocation explicite implémentée : `allocate_denial` et
  `publish_format_denial` passent par une autorité fermée conservant le même
  propriétaire. Toutes les frontières physiques restent vérifiées. Nouveau
  `denial_transition` : source Planned/Copying réellement relue, B et document
  exigés, plan/union/coût redérivés depuis le template Planned immuable,
  destination non enregistrée. Binding Copying avec replay sans writer,
  rollback/close/readback stricts. C SQL reste vide ; registration/copie à venir.
- Lectures managed_journal désormais strictes, annulation comprise. Préflight
  roundtrip intent+body avant B ; commentaire metadata-floor corrigé.
- F3 natives renforcées : ImmutableSource courant et range-watcher armé au
  close famille après COMMIT, pin effectivement vérifié, préimage absente,
  floor metadata−1 distinct du total, plan dérivé altéré, véritable Staging
  via writers capture. Deux nouvelles natives allocation/FORMAT (dont ACK
  SourceCopying retenu). F2 succès/replay vérifient directement le cleanup du reçu.
- r154 : cinq pures PASS après revue. Preuve `provider-c13-history-20261102-r154/c14-sql-results.json`.
- r157 : revue Opus terminée sans produit HIGH/MEDIUM. Quatre receipt DATA/FS
  PASS ; première native orphan recovery FAIL à prepare, avant financement.
  Cause démontrée en source : descriptor README `single()`, préparation publique
  `fixed1MiB`, donc ManifestId exact absent malgré FileId égal. Aucun fixture
  rouvert. Les douze codec/SQL indépendants, non exécutés au premier arrêt, ont
  ensuite tous PASS dans un runner séparé sur racines fraîches. Preuves
  `provider-c13-history-20261102-r157/c14-sql-results.json` et
  `c14-sql-followup-results.json`. Aucun F2 natif complet qualifié à ce point.
- Correction fixture : mode explicite descriptor-only (README fixed1MiB AVANT
  bootstrap, immutable distinct, vraie préparation des deux, puis edit README,
  pin exact sans alias) et native dédiée de reprise orpheline. Mode alias conservé
  séparément. Le produit continue de refuser une préimage dont seule une autre
  représentation est résidente sans producteur courant : aucune relaxation.
- r158 compile E0308 proof non Clone ; correction copie DATA par codec et
  validation exacte. r159 PASS54,24s ; **r161 PASS1m05s**,85 warnings.
  r160 était un gel intermédiaire non exécuté/non revu ; r162 le remplace pour
  qualification. r162 :730 sources,sept chemins depuis r157,PDB age168 ; host
  `83fd68ad4af15e8ab27ab2a033378260dee057e9a4c59521509aa1b4d3928d34`, EXE
  `09c3a03c51624e16e0dcc7bda266e93002c112b8e380002c50ac9e7fdfcc0e60`.
  27 candidates compilées non exécutées ; 16 r157 PASS réutilisés sans répétition.
  Brief `D:/Layrs/tmp/c14-allocation-review-brief-r162.md`.

À faire ensuite : revue du delta allocation, qualification sur fixtures neuves,
puis `register_and_copy` denial-aware sous source/image/B réellement retenus,
utilisant `destination_coverage::read_source` + `initialize_empty_with_current`.
Ne pas retirer les gardes legacy. `PendingDenialStart` conserve le template
Planned ; ses scopes explicites lisent séparément la phase actuelle.

Lacunes encore ouvertes r157 : négatifs complets recovery (reçu absent, intent
en conflit, corps sans intent, seconde réservation), probes fichiers non
observées, pin share-all du reçu détectant sans prévenir suppression externe,
OS-close/crash/power-loss non qualifiés. F7 GC/rétention et capacité C physique,
historique Published/Reserved, lookup authentifié d'une représentation résidente
alternative si requis, caller client/autorité distante, Ready/admission/autonomie
C14 puis reste L0–L9 demeurent ouverts. Le collector client est une observation,
pas un lease serveur retenu jusqu'aux COMMIT.

## Point courant — 2 novembre, gel r157 (prévaut sur l'historique)

**Goal actif ; C14 et le plan global restent incomplets.** Aucun runtime actif
à cette note. Aucun commit/push ; stash original conservé.

- r152 : revue Opus terminée, F1/F4/F5/F6 tenus en source. Sélection élargie
  aux régressions des lecteurs (N2), **33 DATA/SQL PASS**, runner session54916
  terminal exit0. Preuve : `.layrs-local/validation/provider-c13-history-20261102-r152/c14-sql-results.json`.
  Cette preuve reste liée à l'ancien EXE, pas aux sources suivantes.
- r154 : revue Opus terminée, F3 fermé au niveau source pour l'admissibilité
  logique du plan C avant B. Aucun nouveau défaut produit HIGH/MEDIUM trouvé.
  Lacunes à combler : fichier courant immuable et absence de range-read après
  B, refus préimage non résidente, négatifs discriminants et capture pending
  atteinte réellement. Capacité physique C, historique Published et GC ouverts.
- F2 intégré : reçu compact `pending-denial-start.json` publié avant B, lié
  aux A/P et inode réels ; il donne seulement une preuve de libération. Nouveau
  recovery opener distinct de l'Active normal, sans revalidation du checkout.
  Snapshot privé complet retenu : transaction+document absents → release
  exacte de B ; Planned réel exact → garde B. Close positif et readback avant
  cleanup du reçu. Trois natives authored : perte du propriétaire+edit checkout,
  Planned conservé, ACK de release refusé puis reprise sans writer.
- r152 N1/N3/N4/N5/N6 corrigés : garde native après vrai Planned, gardes stage
  dès entrée, transport limité au vrai SourceCopying et preuve C exacte,
  assertions de causes discriminantes. Famille B reste au futur caller.
- Compilation r155 : E0277 conversion d'erreur du verrou ; corrigée.
  **Compilation r156 PASS,53,72s**,85 warnings. Gel r157 :729 sources,
  20 chemins changés depuis r154, PDB age166 ; host SHA256
  `79b9eab5e11344f5d14c3e3207a40bbc69a211066e7ad023e5c518a857052b67`, EXE
  `101df159393caa41d4e6407e675307ef384e3d7aff79252791874f4cd345345b`.
  27 recettes candidates compilées, **aucune exécutée dans ce lot**.
  Brief : `D:/Layrs/tmp/c14-recovery-review-brief-r157.md`.

Prochaine étape : revue critique F2 intégrée avant qualification, puis tests
affectés sur racines fraîches, arrêt au premier échec. Ne pas rejouer toute
l'histoire. Le reçu qualifie au mieux l'interruption de processus ; aucune
garantie power-loss/directory-flush. Orphelins anciens sans reçu refusés, aucun
temp adopté. F7 rétention/GC, caller public et C14 Ready/admission/autonomie
restent à terminer, puis le reste L0–L9.

## Travail en cours — après le gel r152

Le gel r152 reste immuable et sa revue Opus est en cours. Travail vivant F3 :
entrée Capture distincte dans `transition_reconcile/source_sql`, reconcile
Capture retenu sans rehash sous gates, nouveau `managed_journal/denial_plan`
avec constructeur fermé et reconstruction du plan complet. `PendingDenialStart`
retient maintenant observation et union proposées, refuse le plan non admissible
avant réservation, épingle seulement l'union redérivée et répète les comparaisons
dans les scopes SQL avant effets. Kernel C14/selection et sizing prospectif en
intégration parallèle. Les natives utilisent désormais une union réelle avec
préimage historique rendue résidente par préparation publique, et des refus
quota zéro/manifest omis/observation changée. **Compilation r153 PASS,43,30s**,
85 warnings ; aucun test exécuté. Gel r154 de 723 sources,14 chemins changés
depuis r152, PDB age165 ; host SHA256
`7cfdff2632bc3c530c2610f5dc3fd87cfb84ab35563b51b548c336da6437f943`, EXE
`7e35a6b28feaf5b2cedec4601853e61ca397458c6a6590a8cc8295a0398bd621`.
Cinq recettes pures et trois natives sélectionnées. Brief de revue ciblée :
`D:/Layrs/tmp/c14-plan-review-brief-r154.md`.
F2 récupération durable n'est pas implémenté ; aucune activation produit.

## Point courant — 2 novembre, intégration après revue r149

Cette section prévaut sur l'historique ci-dessous. **Goal actif ; C14 incomplet.**

- La revue Opus r149 est terminée : `D:/Layrs/tmp/c14-pending-start-critical-review-r149.md`.
  F1 confirme un défaut : financer B avant de vérifier l'absence de capture
  en attente et les bornes prospectives peut bloquer définitivement Planned.
  Correctif partagé lecteur/préflight et recettes de régression intégrés,
  compilation réussie ; runtime et contre-revue en attente.
- F5 : remplacement des lectures familiales non strictes dans ce démarrage,
  afin de ne pas reprendre de snapshot après erreur. Intégré, à qualifier.
- ROOT a ajouté les gardes C14 aux anciens chemins allocation/observation/
  binding/Staging (F4), avant leurs effets. La lecture du financement réel
  est séparée du contrôle de capacité prospectif (F6), pour préserver
  l'interprétation de l'état durable après baisse du quota.
- Transport courant écrit : `destination_coverage`, initialisation atomique
  avec le document, lecteurs EmptyIngress/CopiedPrefix/Staging et inventaire
  exact hérité + courant. Rejeu sans réparation ; gardes Ready conservées.
  Quatre recettes SQL de transport, trois d'inventaire et une d'intégration
  ajoutées ; **non exécutées** à ce point.
- Compilation r150 : échec E0308 dans `require_destination`, conversion
  d'erreur corrigée. **Compilation r151 PASS en 29,77 s**, 86 warnings.
  Gel r152 : 719 sources, 21 chemins changés depuis r149 ; EXE SHA256
  `05f01390cac1cf27585010fcde1bcb885b49ca66114ec580db21eee8e9eb3afd`,
  PDB age164 ; host SHA256
  `07e7b5753829bed982ff198f29062fffce38ddd22be37557ac749fae8b8e5286`.
  Vingt recettes DATA/SQL sélectionnées, trois natives de démarrage à qualifier.
- F2 reste ouvert : le contexte perdu entre réservation famille et Planned
  nécessite une preuve durable de reprise/libération exacte. F3 reste ouvert :
  le plan C complet et son coût courant doivent être redérivés avant financement.
  F7 : contrat de rétention/GC des réservations encore à définir avant caller public.
- Les PASS r149 ci-dessous sont des résultats réellement exécutés mais
  antérieurs à la revue indépendante ; le prérequis de revue annoncé par
  leur host-disposition n'a pas été respecté. La revue ne les autorise pas
  rétroactivement et ils ne qualifient pas les corrections en cours.

Prochaine étape : revue ciblée r152 avant nouvelles exécutions, brief
`D:/Layrs/tmp/c14-current-review-brief-r152.md`. Aucun runtime actif à cette note.
Pas de commit/push ; stash original préservé. Le reste L0–L9 reste à accomplir.

Piste F2 étudiée en lecture seule : reçu compact de libération, publié avant B
dans provider-control, avec binding exact/opération/coût recomputables et inode
propre ; jamais le gros corps avant financement. Une entrée de récupération
distincte est nécessaire : la réouverture Active ordinaire refuse déjà l'orphelin.
Sous owner natif/A/P/binding/attempt et gates exclusives, retenir le lecteur source
complet prouvant transaction ET corps absents, supprimer seulement la réservation
exacte sans installation, fermer positivement, relire strictement puis enlever
le reçu épinglé. Ne pas exiger l'image actuelle (elle peut avoir changé). Si le
corps/intent est durable, conserver B et seulement régler le reçu après preuve
réelle. Ne jamais convertir ce reçu compact en proof de financement ou autorité
d'insertion. Le mécanisme PendingReceipt existant aide mais ne qualifie pas la
perte de courant ; les anciens orphelins sans reçu restent sans preuve de release.
Cette piste est une conception, pas une récupération implémentée ou qualifiée.

Piste F3 étudiée en lecture seule : `prepare` doit recevoir le vrai
`TransitionObservationData` et passer par un reconcile Capture de l'image complète
(ajouter une entrée source_sql Capture sans fabriquer un proof Publication).
Réutiliser `OwnedCompanionData::derive/require_union`, puis un kernel C14 explicite
avec coverage et digest de sélection de l'observation ; le kernel v1 reste strict.
Redériver baseline à metadata zéro, extraire l'enveloppe depuis quota, calculer
`DeniedCoverageCost` et le floor incluant le courant une fois, redériver avec
l'enveloppe et comparer tout le plan. Ajouter un sizing prospectif DATA :
`destination_intent(Source Planned)` produit une phase interdite et ne peut servir
au calcul avant allocation. Épingler seulement l'union redérivée après admission,
puis refaire les comparaisons dans le scope SQL précédant la réservation B.
Pas de cycle : le corps ne dépend ni du quota ni du digest final du plan.
F3/TG3 restent non implémentés ; ne pas prendre les fixtures quota0 pour un plan C admissible.

## Point courant — 2 novembre, C14 démarrage financé r149

Cette section prévaut sur les suivantes. **Goal actif ; pas d'acceptation globale.**

- Le gap same-owner « réservation B durable / Planned absent » est maintenant
  raccordé par `managed_journal/denial_start.rs::PendingDenialStart`. Contexte
  retenu avant la première mutation, image et pins offline conservés, admission
  complète source/prefix/famille ; réservation exacte avant corps+Planned.
- Le lecteur familial possède une entrée prospective explicite. Les entrées
  existantes restent strictes. Seule l'opération liée à l'allocation/plan/corps
  exacts est reconnue ; le proof réel reste dérivé exclusivement des lignes SQL.
- La réservation déjà durable se rejoue après lecture complète et fermeture
  positive, sans writer familial. Planned déjà durable se rejoue sans aucun
  writer. Les erreurs de fermeture interrompent la chaîne avant toute nouvelle
  sonde ; les scopes existants sont fermés.
- r146 compilation PASS. r147 n'a pas de résultat terminal exploitable : sa
  session a disparu et aucun processus Cargo/rustc/test n'était actif à la reprise.
  **r148 compilation PASS, 41,05 s**, puis gel r149 de 714 sources.
- **r149 : huit SQL PASS et deux natifs PASS** : parcours normal 9,277 s ; deux
  refus d'ACK successifs famille puis B et replay final sans writers 9,023 s.
  Les observateurs confirment zéro probe après ACK refusé et zéro ouverture B
  après refus familial. Les témoins des DB confirment les phases sans écriture.
  Aucun runtime actif après session ROOT58524 terminée code0.
- Gel : `.layrs-local/validation/provider-c13-history-20261102-r149`.
  Host SHA256 `ecb29bbc8f4a85622c4f0a1392cc3151cbbdf133a804c3025e45b9d21c9a08d3` ;
  EXE SHA256 `c4d216bea6c03521f88c622481aae6e8010d0200329102a9f097ca5e5632a13b`,
  PDB age163. Résultats `c14-sql-results.json` et `c14-native-results.json`.

Limites : observation locale normalisée de fixture, pas de lease serveur ; un
fichier natif courant et aucun C créé ; ACK retenu après fermeture SQLite réussie,
pas panne OS/crash ; reprise avec même owner uniquement. Le nouveau démarrage
n'est pas exposé par un caller produit. Les tests ne qualifient pas Ready/admission.
Les revues r139/r142/r145 n'ont fourni aucune acceptation indépendante (limite
fournisseur puis notifications de reprise annulée). Nouvelle revue intégrée r149
à déléguer sur le gel, sans demander de réexécuter les tests ROOT.

Prochain travail : transport du corps courant (distinct des hérités déjà copiés),
inventaire exact destination et propagation du coût réel dans les dérivations
du plan/Ready/admission/autonomie ; puis raccordement du vrai caller avec autorité
retenue. Préserver les refus v2 des modes encore non pris en charge. Le reste
du plan L0–L9, dont client/CLI, reprise fresh-owner et qualification finale, demeure.
Pas de commit/push ; stash original préservé.

## Historique — 29 octobre, C13 r142 / C14 r145

Cette section prévaut sur les suivantes. **Goal actif, aucune acceptation globale.**

- **C13 r142 PASS natif, 590,105 s**, session ROOT74117 terminée code0.
  Le fichier inchangé devenu Reserved est conservé en compagnon, C est publié
  puis rouvert sans owner B par `ManagedReadyInspection`. Correction de
  C13R136-001 ; variante Tree couverte en DATA seulement. r140 : 19 DATA PASS.
  Les trois natifs r136 restent PASS (629,724 / 679,830 / 63,420 s) ; pas de
  réexécution inutile de ces parcours sur les modifications C14.
  Preuve : `.layrs-local/validation/provider-c13-history-20261029-r142/c13-native-results.json`.
- **Contre-revue C13 r142 et revue transport C14 r139 : FAILED**, limite API
  Claude. Aucun résultat de revue ni acceptation indépendante. Ne pas poller
  ces tâches terminales, ne pas substituer un autre modèle sans demande.
  Nouvelle revue du financement C14 r145 également FAILED sur la même limite
  (`41f2669c-6eb6-43a5-86d9-e079f155281a`). Aucun reviewer actif ; poursuivre
  l'intégration autorisée sans nouvelle boucle de tentatives Opus immédiates.
  Le message fournisseur précise : limite de session, réinitialisation annoncée
  à **17 h Europe/Paris le 29 octobre**. Ne pas retenter avant cette échéance.
- **C14 r139 : cinq DATA et neuf SQL PASS**, transport des documents hérités
  sous prefix-v3, corps réels liés aux observations et refus legacy v2.
  Preuves conservées dans le gel r139 ; l'activation C14 reste fermée.
- **C14 financement r145 : compilation r144 PASS (22,63 s), huit DATA et cinq
  SQL PASS.** Nouveau `shared/source_denial` : réservation B fixe, dérivée du
  corps complet et de l'allocation/plan/famille exacts, indépendante de la
  phase et de C physique. Coût = colonnes document réelles + borne codec intent
  B et scalaires + scalaires réservation. Rejeu exact, pas de débit/réparation.
  Les lecteurs Transition/Publication reconstruisent les bindings depuis les
  corps SQL effectifs ; le classifier familial exige les montants présents.
  Le test Planned/Cancelled utilise deux bases SQL jetables et le vrai proof
  source pour refuser absence, montant altéré et rollback du financement.
  Les trois autres tests de réservation couvrent capacité exacte/moins un,
  comptabilité globale, collisions et préconditions transaction/FK/cancellation.
  Le floor C possède un coût courant fermé dérivé du corps réel ; les anciens
  callers refusent v2 sans ce coût. Les hérités restent dans copied metadata.
  Aucun caller actif v2, aucune preuve native ou atomicité inter-bases revendiquée.
- Gel r145 : 712 sources, EXE/PDB age161. Host SHA256
  `77c8cc7127470098104456ca2b610646dffdf3523f025287b691aea4e9023c71` ; EXE SHA256
  `106a8b91fd51f689bf1f90aea9db62e3dda6d77434b7b8ebd4b0cc44d3e955fc`.
  Résultats : `.layrs-local/validation/provider-c13-history-20261029-r145/c14-data-results.json`
  et `c14-sql-results.json`. Aucun runtime actif à ce point.

Prochaine intégration concrète : gap réservation B durable / Planned absent.
Exiger un contexte prospectif exact retenu par le même owner, sans accepter un
namespace seul ; COMMIT et fermeture familiale positive avant writer B ; arrêt
immédiat après ACK incertain ; corps+Planned atomiques puis full readback.
L'investigation de raccordement `c14_derived_storage_guard` est terminée, lecture
seule. Proposition et points à trancher :
`D:/Layrs/tmp/c14-pending-denial-start-handoff-r145.json`. Retenir les pins offline
avant réservation ; ne pas repasser par une admission familiale stricte au retry
du gap. Si Planned existe déjà après ACK B refusé, utiliser le vrai proof complet
et non le contexte prospectif. Le fresh-owner sans corps demeure hors prise en charge.
Ensuite : vrai caller, document courant/inventaire destination, propagation du
coût, Ready/admission/autonomie C14 ; client/CLI et reste L0–L9 toujours à finir.
Pas de commit/push, stash original préservé. Les tests ciblés ne valent pas
acceptation globale ni qualification crash/échec OS de fermeture.

## Historique — après revue C14 r134

Cette section prévaut sur les états historiques ci-dessous. Goal actif.

- **C13 r136 : premier natif PASS, 629,724 s.**
  `c13_real_accepted_edit_denied_present_companion_reaches_c_ready_and_b_published`
  a terminé avec code0. Le runner ROOT65400 poursuit le scénario same-owner
  à onze points de refus d'ACK, puis le cas de préimage froide manquante.
  Ne pas démarrer Cargo/SQL/natif tant que cette session n'est pas terminale.
  Preuve : `.layrs-local/validation/provider-c13-history-20261029-r136/c13-native-results.json`.
  Aucun résultat final des deux autres recettes n'est encore revendiqué.
- Revue Opus C14 r134 terminée : aucun défaut Critical/High/Medium ; deux Low.
  Rapport `D:/Layrs/tmp/c14-source-critical-review-r134.json`. F1 : des lecteurs
  legacy pouvaient décoder v2 sans exiger son corps ; F2 : test policy/epoch absent.
  ROOT a ajouté les refus v2 explicites aux lecteurs destination, Ordinary et
  admission, et exige prefix-v3 pour un reçu v2 hérité dans ancestry. Nouveau
  test policy/epoch avec corps et référence recalculés. Ces changements restent
  **non compilés/non exécutés** à ce point ; ne pas transférer les PASS r134.
- Transport C14 hérité intégré par GPT-6.1-Sol : rowset14 DenialObservations,
  domaine distinct, engagement des documents complets triés, bornes existantes
  1M lignes/1GiB et 64MiB/document avant allocation. IDs issus exclusivement
  des reçus entrants authentifiés ; document du transfert courant exclu pour
  éviter le cycle de digest. Copie avant les reçus, replay exact et collision
  sans réparation. Ordinary6 autorise le scope vide sans table supplémentaire.
- ROOT a raccordé `require_observation_rows` et `transition_observation/stored.rs` :
  liaison aux lignes snapshot/sélection authentifiées, génération/digests,
  Step/root, policy/epoch et sémantique ; recontrôle du namespace visible/refusé
  partagé, aliases, ancêtres, frontières et scope. Aucune autorité distante créée.
  Deux tests DATA ajoutés (policy/epoch et namespace historique).
- Sept fichiers transport, six recettes (une DATA, cinq SQL) préparés par l'agent,
  rustfmt/check et diff-check PASS seulement. Compilation ROOT puis DATA ciblées,
  cinq SQL transport et régressions legacy concernées à exécuter une fois r136
  terminal. Revue C13 r136 Opus encore en attente, ne pas la sonder.
- C14 n'est pas activé : restent caller réel, financement complet des documents,
  document courant et inventaires source/destination, Ready/admission/autonomie.
  Les gardes correspondantes restent fermées. Aucun commit/push ni acceptation globale.

## État courant — C13 r121 / intégration C14, 29 octobre

Cette section remplace les mentions « en cours » ci-dessous. Goal toujours actif.

### Dernière suite r122–r129 (prioritaire)

#### Suite r130–r134 — état le plus récent

##### Suite r135/r136 — prioritaire

- r134 natif terminal **FAIL 23,764 s**, session ROOT33027 code1. Le parcours
  dépasse les fixtures précédentes et révèle un défaut produit dans
  managed_plan::derive_kernel : un fichier Reserved présent, déjà retenu par
  le compagnon, était encore exigé dans l'image de checkout C.
- ROOT corrige : seul un owner reconciled typé autorise une entrée Reserved,
  sans writable, sans fichier checkout et sans suppression. Le kernel legacy
  garde le refus. Le nouvel exemple DATA dérive réellement observation+owner,
  vérifie compagnon/union/budget, maintien Reserved et absence de Capture.
- r135 compilation **PASS 19,00 s** ; 13 managed-plan DATA **PASS 0,120 s**,
  10 companion DATA **PASS 0,360 s**, incluant le nouveau cas. Le premier
  sélecteur de groupe companions était erroné : aucun test de ce groupe lancé
  avant correction du nom en companion_codec_recipes (r135b), sans rerun des13.
- R136 gel vérifie exactement deux changements depuis r134 : managed_plan.rs
  et transition_reconcile/tests/companions.rs. Tous les fichiers C14 sont
  inchangés et restent sous la revue Opus r134 (task suffix9a204f63).
  Rétention `.layrs-local/validation/provider-c13-history-20261029-r136` ;
  host SHA256 `4022e176dc18d13e4513be2df21509b848b2d8235aca0b100ca3984a7bd8f9bf` ;
  EXE SHA256 `6bedf9cac4fae000e8936a50a0bccba39e7cb9487be6cd1b1b290de5c30e5299`.
  Runner natif r136 lancé, session ROOT65400, série3, stop premier échec. Aucune autre exécution
  Cargo/SQL/native tant qu'il n'est pas terminal. Aucune qualification globale.

- Agent source C14 terminé : sept fichiers, lecteur complet Provider des
  documents v2 et primitive apply_source_intent_with_coverage. Documents
  dérivés + Planned dans la même transaction main Write ; replays et phases
  exigent le corps SQL présent, jamais réparé. Préflight 4096 documents,
  64MiB chacun, inventaire exact et comparaison aux semantics/Step/root actifs.
  Bootstrap/Preparation/Active/ActiveCapture restent stricts ; prefix-v3 refusé.
  Caller réel, destination/transport, Ordinary et financement restent à faire.
- r130 compilation FAIL : helper digest_token privé et valeur cancelled
  déplacée dans test. ROOT utilise raw_digest après strip_prefix, comparaison
  empruntée dans le test. r131 compilation PASS 29,02 s.
- r132 : sept SQL PASS (3 nouveaux writer, 4 historiques intention/publication).
  Test preflight FAIL car STRICT rejetait déjà TEXT dans BLOB malgré
  ignore_check_constraints. Attente corrigée pour vérifier ce refus effectif.
  r133 compilation PASS 15,27 s. r134 quatre SQL restants PASS
  (0,123 / 0,422 / 0,119 / 0,271 s). **Onze SQL ciblés acquis** ; les sept premiers
  conservés avec preuve que seul le test preflight a changé. Workspace all-targets
  **PASS 14,39 s**. Aucun autre rerun nécessaire pour ces sources inchangées.
- Rétention r134 : `.layrs-local/validation/provider-c13-history-20261029-r134`,
  705 sources, EXE SHA256
  `1df62aa4b0dd182e1ef13f1a58e260715058bf2a331131bc41987b793dc89f35`.
  Host JSON SHA256 `3d2f29918a3e5905e7b8ba04d5440b68d33a47fca705207291e068bfb21f863d`.
  Lire `root-current-disposition.json` pour la portée actuelle : les phrases
  historiques cumulées dans host-disposition.scope ne décrivent pas toutes r134.
- ROOT a autorisé les trois natifs C13 diagnostiques r134 après ces contrôles.
  Runner lancé en série, nouvelles fixtures C:, stop premier échec. Revue critique
  indépendante de l'intégration SOURCE C14 à lancer sur ce gel. Aucune acceptation
  globale ou qualification C13/C14 ; goal reste actif. Ne pas démarrer un autre
  Cargo/SQL/natif tant que ce runner n'est pas terminal.

- Deux agents GPT-6.1-Sol ont terminé les durcissements F1/F2/I3 : wrapper
  DerivedDeniedCoverage scellé, garde SQL main Write/query_only, budget commun
  fresh-visible+denied (sans compter deux fois les sélections/descripteurs).
  r122 compilation **PASS 38,54 s** ; r123 DATA : 7 tests record distincts PASS
  (accidentellement compilés deux fois), 16 observation PASS, 32 codecs PASS.
  **Cinq SQL PASS**, notamment BEGIN DEFERRED/savepoint/query_only sans écriture.
  Artefacts : `.layrs-local/validation/provider-c13-history-20261029-r123`.
- ROOT a corrigé le chemin explicite de binding/tests.rs. r124 compilation
  PASS 23,79 s. r125 : test de reconstruction PASS ; test de substitutions FAIL
  car le token substitué rootTreeId était identique à la fixture. Le test choisit
  désormais une valeur différente et vérifie que la mutation est réelle.
  r126 compilation PASS 16,29 s ; r127 **deux tests de binding PASS** (0,072 s).
- r127 natif C13 **FAIL 26,478 s**, même assertion Source, sans lancer les deux
  suivants. Diagnostic précis dans layrs-vfs/evidence.rs : Immutable exige
  partial+in_sync+modified_bytes=0 ; un fichier entièrement hydraté est NeedsHash
  même readonly. ROOT a laissé unchanged.txt froid côté placeholder ; ses octets
  sont déjà présents dans CAS par la préparation publique. Aucun garde produit
  ni assertion affaibli. Anciennes fixtures r121/r127 laissées intactes.
- r128 compilation PASS 14,25 s. Gel r129 vérifie un seul fichier modifié depuis
  r127 (fixture C13), 702 sources, EXE SHA256
  `9d12e2950659fcc7c4dd0f29df3177c03dfa6997620160c9379cc589e186e212`.
  Runner `tmp/provider-c13-history-native-run-r129.py r129` terminé code1,
  session ROOT29619 : **FAIL 26,230 s**. L'assertion Source est désormais passée ;
  le préfixe préparatoire échouait car son count captures était encore 1 pour
  deux Captures. ROOT a remplacé les compteurs préparatoires par ceux de la preuve
  réelle (génération, captures, descripteurs, mappings). Non recompilé. Les deux
  autres recettes n'ont pas été exécutées.
- Agent `c14_derived_storage_guard` travaille désormais sur l'intégration
  Provider SOURCE : lecteur complet des documents, inventaire exact des v2,
  nouvelle primitive Planned atomique. Ownership validation.rs/transition.rs,
  transition_sql.rs et tests/helper dédiés. Aucun caller actif, transport,
  Ordinary ou destination autorisé dans ce lot. Prefix-v3 reste refusé tant
  que le transport n'est pas raccordé. Aucun build ni test chez cet agent.

**Aucun runtime en cours. Attendre le handoff de l'agent avant prochain build.**
Les éditions source C14 peuvent continuer. Aucune qualification C13 n'est encore
acquise ; ne rouvrir aucune fixture précédente.

- Revue Opus r109 terminée : les deux blocages de reprise r102 sont corrigés
  en source. ROOT a corrigé le refus des représentations Capture historiques
  et déplacé la fixation des pins après validation de la publication. Les
  recettes couvrent désormais onze ACK retenus, les contrôles négatifs hors
  driver, une Capture remplacée et les producteurs de préimages hors ligne.
- Revue Opus r115 terminée : aucun défaut critique/haut/moyen, deux faibles
  (construction non scellée du document, transaction non garantie en écriture).
  Garder le cycle acyclique : prefix-v3 ne contient que les documents hérités.
  Les lecteurs actifs, l'écriture atomique Planned et le transport restent à
  raccorder ; les barrières historiques n'ont pas été levées.
- r118 compilation lib **PASS 41,07 s**, workspace all-targets **PASS 20,28 s**.
  r116/r117 avaient révélé deux erreurs locales du nouveau helper de liaison
  (chemin de module et conversion de résultat), corrigées avant r118.
- r119 codecs : transition 9 PASS, import 22 PASS / 1 FAIL. Cause vérifiée
  dans HEAD : ancien test rejetant Ordinary10, alors que 9 et 10 sont admis.
  Test corrigé avec cas positifs 9/10 et refus 6/7/8/11 ; aucun garde produit
  affaibli. r120 compilation **PASS 13,00 s**.
- r121 **32 codecs PASS** (9 transition, 23 import), artefact/699 sources
  conservés sous `.layrs-local/validation/provider-c13-history-20261029-r121`.
  EXE SHA256 `ee2b87b56351e7eca367424fbc281156f50b44e602a4694d811754a4808c4180`.
  Le premier natif C13 a réellement été exécuté : **FAIL 24,365 s**, assertion
  « unchanged file must exercise the immutable Source reader ». Fixture
  incorrectement writable ; corrigée à false. La représentation de la Capture
  historique a aussi été ajoutée à l'inventaire fourni et liée au mapping
  effectivement relu. Ces deux corrections ne sont pas encore compilées.
  Runner arrêté au premier échec ; deux autres natifs NON EXÉCUTÉS. Session
  ROOT96504 terminée code1. Ne pas rouvrir sa fixture, ne pas relancer r121.
- C14 fermé : codecs intent/receipt v2 et prefix v3 intégrés et testés ;
  producteurs encore legacy/None. Helper require_intent/require_observation
  compilé r118 ; ses deux nouveaux tests sont écrits, pas encore exécutés.
- Durcissements C14 en intégration : DerivedDeniedCoverage scellé, writer
  exigeant une transaction main Write et query_only=0 ; fixture dérivée du
  normaliseur et nouveau test SQL. Agent terminé, format vérifié, non compilé.
  Agent de budget commun fresh-visible + denied encore en cours.

Prochaine action : intégrer les handoffs, compiler une seule fois, retenir le
nouvel artefact, exécuter les DATA/SQL affectés puis les natifs C13 avec de
nouvelles fixtures. Aucun build/runtime en cours à cet instant. Ne pas confondre
les revues source et les PASS de primitives avec une qualification C13/C14.

## Suite active — correctifs de reprise C13 r109, 29 octobre

Cette section remplace les états « en cours » des sections historiques suivantes.

- Suite r111/r112 : le cas natif de préimage originale absente est maintenant
  écrit et compilé (pas exécuté). Il exige une cause I/O NotFound, zéro nouveau
  writer, les trois bases byte-identiques et aucun appel d'hydratation implicite.
  r110 a détecté l'emploi incorrect de DatabaseFile comme Path dans le témoin ;
  ROOT a utilisé son accesseur path(). r111 compilation **PASS 23,80 s**.
- R112 **trois SQL PASS** (0,069 / 0,069 / 0,071 s) : réservation fixe B+C,
  namespace de racines parcouru progressivement, insertion/rejeu de suppression.
  Artefact et 691 sources retenus sous
  `.layrs-local/validation/provider-c13-complete-20261029-r112` ; EXE SHA256
  `22edb66e2882c151ef2a2d41a0613cd1f7f3ca87376d611ec89d1a5b58e16ca0`.
  `c13-sql-source-release.json` autorise uniquement ces trois primitives et
  `c13-sql-results.json` contient les résultats. Aucun natif C13 exécuté.
- Contre-revue Opus5.5/xhigh r109 en cours : taskId
  `node:delegated-task:command%3Amcp%3Abcb659e8-99b8-40bf-8ee3-f3319081559e%3Adelegate-task%3A9f84f9dc-1161-462a-9eb0-9c094585fe6f`.
  Ne pas poller ; résultat asynchrone. Gel r109 inchangé ; le seul delta r111
  est la recette froide dans c13_complete_ready_tests.rs, retenue avec r112.
- Agent natif GPT-6.1-Sol `c14_codec_storage_integration` chargé d'intégrer les
  seuls codecs/primitives de stockage C14 et leurs déclarations de modules.
  Pas de changement des lecteurs stricts ni de raccordement actif C14 à ce stade.
  Intégration terminée : quatre nouveaux fichiers, deux déclarations étroites.
  r113 compilation a révélé une durée de vie manquante de snapshot dans
  require_shared_namespace ; ROOT l'a corrigée. r114 compilation **PASS 48,22 s**.
  Sept tests DATA **PASS** via `tmp/run-c14-data-r114.py`, ROOT28470 terminal0 ;
  tests 25,54 s, dont 100000 chemins à exactement 8MiB et refus du dépassement.
  résultats sous `.layrs-local/validation/provider-c14-data-20261029-r114`.
  R115 : **quatre tests SQL C14 PASS**, 0,071 / 0,070 / 0,120 / 0,070 s.
  Insertion/rejeu/rollback, refus d'un corps substitué/non canonique, refus du
  BLOB surdimensionné avant décodage, transaction/FK/annulation sans effets.
  Artefact et 695 sources : `.layrs-local/validation/provider-c14-primitives-20261029-r115`.
  Le contexte actif/Planned, Ready, transport et barrières d'acceptation ne sont
  pas raccordés par ces tests de primitives.
- Cartographie C14 par explorer `c14_versioned_links_map` terminée : garder
  les modèles internes communs mais employer des wire types fermés séparés pour
  intent/receipt v1/v2 et prefix v1/v2/v3, avec champs obligatoires non nuls dans
  les nouvelles versions et domaines de digest distincts. Pas d'Option tolérée
  sur l'ancien wire, pas de Value/flatten permissif. 10 plan literals, 10 receipt
  literals et 23 closure literals à mettre à jour ; les producers doivent
  sélectionner les nouvelles versions. require_transition doit interdire tout
  changement de schema_version pendant CAS. Prefix v3 compte des DOCUMENTS.
  **Éviter le cycle** : denialObservations du préfixe source inclut seulement les
  documents hérités ; le document de la transition actuelle lie déjà ce préfixe
  et entre dans plan-v2/receipt-v2, puis dans le préfixe du prochain export.
  Les lecteurs devront comparer le corps réel à transaction/source/identités/
  Step/root/epoch/observation et maintenir les refus legacy jusqu'au raccordement.

- Revue Opus r102 reçue et lue : deux P1 de reprise same-owner, un P2 de
  couverture par alias FileId et un P3 de contrôles de test dans le driver.
  Rapports `tmp/c13-complete-ready-critical-review-r102.{json,md}`.
- GPT-6.1-Sol a corrigé le driver dans managed_content : choix de phase durable,
  réemploi des pins, conservation des DATA Ready, reconstruction terminale
  validée sans writer, durée d'emprunt de l'image séparée de celle des pins.
  Les assertions/replays cfg(test) ont été retirés du parcours de production.
- ROOT a ajouté les préimages descriptor-only à la preuve CAS hors ligne avant
  réservation, avec vérification de l'ensemble conservé lors des reprises.
  Le scénario natif utilise maintenant cinq FileIds distincts, dont une source
  inchangée, et des préimages rendues résidentes par préparation publique puis
  hydratation réelle en processus enfant. Il appelle `commit_companion`.
  Une deuxième recette refuse successivement les ACK après installation,
  journal checkout, B Ready, B Published et reprend avec les mêmes pins.
  Ces deux recettes sont compilées mais **NON EXÉCUTÉES**.
- r108 compilation refusée : durée de vie manquante dans un ancien test et
  mauvais argument du callback de préparation dans le nouveau scénario.
  ROOT a corrigé les deux. r109 store-lib-no-run **PASS 41,44 s**.
  Logs conservés `tmp/c13-recovery-lib-no-run-r108.log` et `...-r109.log`.
- Gel r109 : `tmp/c13-recovery-integrated-r109/manifest.json`, 80 fichiers,
  13 modifiés depuis r102 ; SHA256
  `2e85427369269dd49eaa0dcea0b16fb1514e70273a2f9aa6acb21b292401f9e1`.
  Workspace/all-targets ROOT89919 terminal0 **PASS 24,85 s**, log
  `tmp/c13-recovery-workspace-check-r109.log`. `git diff --check` PASS.
- Collecteur client r107 : compilation **PASS 19,79 s**, puis **21/21 tests TLS
  locaux PASS en 2,45 s**, aucun serveur de production. Résultats et bindings
  `.layrs-local/validation/provider-observation-20261029-r107/`.
- À faire immédiatement : contre-revue Opus des correctifs, finir qualification
  SQL/native C13 et ajouter le refus natif d'une préimage froide absente. Aucun
  résultat C13 natif ni correction runtime encore revendiqué. C14/host/CLI et
  programme L0–L9 restent ouverts. Goal **active**, aucun push ni nouveau commit.

## Suite active — C13 compilé, données vérifiées, collecteur client intégré

- ROOT49288/r102 terminal0 : store-lib-no-run **PASS46,505s**, workspace
  all-targets **PASS26,034s**. Aucun runtime natif/SQL C13 encore exécuté.
- Revue Opus5.5/xhigh démarrée via claudeAgent, aucune demande d'approbation :
  taskId `node:delegated-task:command%3Amcp%3Abcb659e8-99b8-40bf-8ee3-f3319081559e%3Adelegate-task%3A7eb42b45-f356-492e-bbee-b7ed582c12d8`.
  threadId identique avec préfixe `thread:`. Ne pas la poller ; attendre son
  résultat. Cible figée r102, sortie prévue
  `tmp/c13-complete-ready-critical-review-r102.json`. ROOT corrige le dépôt,
  jamais le gel r102. Aucun résultat de cette revue encore reçu.
- R103 : EXE/PDB et691sources retenus dans
  `.layrs-local/validation/provider-c13-complete-20261029-r103`.
  EXE88184832octets SHA256
  `f4be3b2a2d5450fe578458f56b874beffc3b6f212ba849c11b3fbe89264618c2`,
  CodeView âge142. Binding artefact seulement, pas de release native/SQL.
- Sélection DATA initiale r103 : mauvais chemin de module, **zéro test lancé**,
  échec correctement conservé. R103b corrige les noms d'après `--list` du vrai
  EXE (`companion_codec_recipes`). Deux PASS puis un échec de scénario invalide.
  R103c exécute les neuf autres DATA indépendants : **neuf PASS** ; aucun SQL.
  Fichiers résultats et logs dans le répertoire retenu r103.
- Le scénario fautif changeait un target sous le même Step/root immuable.
  Première tentative de correction ROOT r104 également refusée comme preuve :
  elle supposait des Captures cumulatives, alors que chaque Capture est un diff
  complet contre sa génération. Build PASS12,810s, test FAILED conservé.
  Aucun changement de règles de production pour accommoder ces mauvais cas.
  R105 remplace ce scénario impossible par
  `c13_v2_accepted_deletion_refuses_changed_immutable_base_and_retains_authorized_diff` :
  refus exact du target substitué, réobservation valide, diff de suppression
  autorisé exact, capture historique inchangée et préimage dans l'union.
  ROOT34891 terminal0 ; build19,49s et test **PASS**. Log
  `tmp/c13-corrected-deletion-data-r105.log`. Seul companions.rs de tests diffère
  du gel r102 côté store. Les11autres PASS restent valides. Ne pas revendiquer
  un cas positif accepted SatisfiedDeletion impossible dans ce contrat same-Step.
- Collecteur client corrigé précédemment revu désormais intégré : trois fichiers
  sous client-core/native_scm/remote. Manifest d'entrée
  `35bae32f11b6d3f31d19d340c886b984484146a31259b6e1522203e5d68b0861`.
  ROOT6674/r106 terminal1, deux erreurs compiler : conversion String→RemoteError
  et tableau temporaire emprunté dans un test. ROOT a corrigé .map_err(core_error)
  et la liaison locale du tableau. Rapport r106 conservé, aucun test TLS lancé.
  **ROOT71776** nouvelle compilation client-lib-no-run r107 en cours ; log
  `tmp/provider-observation-client-lib-no-run-r107.log`. Aucun autre Cargo/SQL/natif.
  Après compilation : tests ciblés du collecteur sur serveur TLS127.0.0.1,
  puis workspace check si nécessaire ; pas d'accès à un serveur de production.
- C14 codecs/stockage restent en brouillon isolé, non intégrés. C13 Store doit
  encore passer SQL/natif après traitement de la revue. Client host/CLI,
  transport durable C14 et suite L0–L9 restent nécessaires ; goal **active**.


Les sections plus anciennes (à partir de « Reprise effective du 29 octobre ») sont dans `worktrees-partial-vfs-resume-archive-2026-10-28.md`.
