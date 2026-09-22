# Revue indépendante du Store partiel — 22 septembre 2026

Périmètre : publication durable, namespace et vérification locale. Le moteur
refresh est traité séparément dans `PARTIAL-REFRESH.md`. Aucun commit effectué
par cet agent ; intégration et commits réservés au parent.

## Corrections réalisées

- Un reçu terminé vérifie désormais sa relation avec la capture d'origine :
  snapshot suivant exact, génération suivante, parent exact, mêmes permissions
  et seuls changements capturés. Un rejet conserve exactement la base d'origine.
  La corruption de `next_snapshot` ne peut plus passer silencieusement dans
  `completed_publication`, un rejeu ou `fsck`.
- La reprise d'un journal namespace refuse un journal refresh simultané avant
  toute mutation des fichiers, comme les journaux installation/publication.
- Un fichier créé localement peut être renommé avant sa première publication.
  Sa source doit rester une absence explicitement observée, writable, avec un
  fichier régulier effectivement présent. Le patch omet la source absente de la
  base distante ; il contient uniquement la création à destination. L'historique
  namespace et fsck vérifient ce cas sans inventer une suppression distante.

## Vérification

- `cargo test --offline --locked -p layrs-store-local --lib projection -- --nocapture`
  : **31 PASS, 2,51 s** (durcissement publication/reprise et tests refresh inclus,
  avant ajout du test final de renommage d'une création locale).
- `cargo test --offline --locked -p layrs-store-local --lib projection::tests::namespace -- --nocapture`
  : **6 PASS, 1,20 s**, dont création puis renommage avant publication.
- Première passe Clippy strict : deux `needless_borrow` dans le lot refresh,
  signalés à son propriétaire ; aucun diagnostic dans les fichiers de ce lot.

## Restauration sélective livrée dans le moteur local

Une restauration explicite de chemins présents dans une capture utilise
leurs remplacements immuables (ou leur suppression), en revalidant les droits
**courants** : chemin connu, lisible et writable, jamais Reserved/Unavailable.
Elle garde la base distante et son epoch inchangées, conserve les octets actuels
en quarantaine et en SQLite, et journalise l'opération avant mutation. Undo/redo
sont réalisables par inversion explicite de chaque reçu de restauration.

API : `preview_restore(generation, RestoreSource::Capture { sequence, paths })`,
`apply_restore(preview)`, `resume_restore(root)`. Une source
`RestoreSource::Inverse { operation_id }` prépare l'annulation exacte des seuls
chemins concernés ; inverser ce nouveau reçu effectue redo. La sérialisation
JSON utilise `operationId`. Le client vérifie les droits serveur frais avant
apply ; le moteur impose génération, digest de checkout, droits observés actuels
et namespace. Le reçu conserve l'identifiant et la capture finale.

Les previews refusent un résultat identique, les chemins inconnus/non capturés,
les changements readonly/réservés et les budgets dépassés. Chaque reprise
vérifie les inodes, les octets, les modes et l'identité du dossier de quarantaine.
Les stages antérieurs au journal restent privés et ne bloquent pas une nouvelle
tentative. Aucun écrasement d'une collision externe ; journal conservé pour
inspection. Les journaux incompatibles bloquent aussi la reprise d'installation.

Deux tables format3 (`projection_restore_pending` et `projection_restore_history`)
isolent ce journal. `fsck` valide maintenant les historiques refresh et restore,
en plus des snapshots, captures, publications et namespace. Aucun format2 migré
implicitement. Les bases de fixtures format3 anciennes doivent être recréées.

Restaurer « tout l'état à cette capture » nécessite aussi les octets de base des
chemins absents du patch. Ils ne sont pas tous conservés localement : le client
doit les obtenir avec des autorisations actuelles, puis préparer les octets avant
le verrou local. Le moteur refresh refuse les modifications locales et ne doit
pas être détourné pour fabriquer une nouvelle base distante. Le partage de ses
primitives de staging/quarantaine est à coordonner avant implémentation.

Qualification finale Windows : **42 tests projection PASS (3,81 s)** ; Clippy
strict Store local `--lib --tests -- -D warnings` **PASS (2,19 s)**. Six tests
restore comprennent six points de coupure (stage, journal, quarantaine,
installation, avant/après commit), undo/redo, suppression, collision externe,
remplacement d'inode à octets identiques, changement de droits et journal
concurrent d'installation. Une première version du test de révocation utilisait
à tort le CAS simple pour changer les droits ; corrigée pour passer par le
refresh journalisé. La dernière qualification est verte.

Qualification Linux par refresh_finish : **6 tests restore PASS (2,27 s)**,
0 ignoré, exit0 ; journal brut `tmp/restore-linux-final-20260922.log`.

Restant à l'intégration parent : parcours client/serveur et UI restore, ainsi que
renommage de créations.
La restauration de l'état complet d'un ancien checkout, les symlinks et les
répertoires restent hors de cette API explicite de feuilles capturées.
