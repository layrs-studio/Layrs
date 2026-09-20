# Pont Git local

Cette crate constitue une frontière explicite. Le stockage et l'admission SCM
natifs ne dépendent pas de Git. Un exécutable Git installé est nécessaire pour
ces conversions ; aucun checkout, filtre de contenu, hook ou accès réseau n'est
effectué par le pont.

```rust,ignore
let plan = scan_git(source, GitBridgeLimits::default())?;
// Dry-run : rapport et objets en mémoire, source inchangée.
println!("{:?}", plan.report());
let imported = plan.install(&native_workspace)?;
let exported = export_workspace(
    &native_workspace, new_destination, ExportMode::History,
    GitBridgeLimits::default(),
)?;
```

L'import installe atomiquement objets, admissions, Layers, Markers, observations,
Operation et vue native avec attentes CAS. Il ne change aucun fichier du
checkout. Son identifiant stable permet une répétition sans doublon après
réouverture. Un changement des références Git pendant le scan provoque un refus.

| Source | Conversion et garantie |
|---|---|
| Fichiers texte, binaires, vides | Octets préservés, mode exécutable conservé |
| Symlinks | Cible UTF-8 conservée comme objet, jamais suivie |
| Chemins | UTF-8/NFC natif, noms avec saut de ligne préservés ; réserves `.git`/`.layrs` refusées |
| Collision de casse | Objets conservés avec avertissement ; matérialisation dépend de la plateforme |
| Commit + parents | Step canonique + observation ; parents canoniques triés/dédoublonnés |
| Plusieurs commits, même Step | Toutes provenances conservées ; coalescence signalée |
| Auteur/date/message/encoding | Objet commit brut exact en provenance base64 ; export repris si provenance unique |
| Branches locales | Layers `git/<nom>` ; collision de nom natif refusée |
| Tags légers/annotés directs | Markers, annotation exacte en provenance ; aucune Release implicite |
| Signatures commit/tag | Provenance exacte ; signatures retirées des objets reconstruits et rapportées |
| SHA-1/SHA-256 | Lecture selon format source ; export vers nouveau dépôt SHA-1 |
| Layer divergente | Chaque tête exportée sous `refs/heads/<nom>/head-<Step hash>` |
| Conflict natif | Historique refusé avec liste des Steps ; snapshot propre explicite possible |
| Tombstone natif | Chemin absent, perte de provenance de suppression signalée |
| Step sans provenance unique | Auteur synthétique `Layrs Export <layrs-export@localhost>`, date epoch 0, avertissement |

Le mode `History` exporte tous les Layers/Markers visibles et leur fermeture de
parents. Chaque capture native peut donc devenir un commit. `Snapshot` exporte
un Step sélectionné sans ses parents, avec une référence fournie explicitement.
Les collisions de références Git, y compris préfixes, sont refusées.

L'export exige une destination inexistante et crée un dépôt **bare**. Le marqueur
`layrs-export.incomplete` reste présent jusqu'à réussite de `git fsck --strict
--full` et écriture de `layrs-export-report.json`. Un échec laisse le dossier pour
diagnostic. `resume_export_workspace` reprend explicitement ce dossier si son
checkpoint correspond exactement au Space, à l'Operation, à la vue et au mode
actuels. Les liens/reparse points, fichiers racine étrangers et références
inattendues ou différentes sont refusés. Les objets sont recalculés et vérifiés ;
les références sont publiées ensemble par transaction Git avec attentes CAS.
Cette reprise ne protège pas contre un autre processus local hostile remplaçant
des chemins après vérification. Les hashes Git identiques ne sont pas une garantie.

`scan_git_with_progress` et `export_workspace_with_progress` prennent en dernier
argument `&mut FnMut(&BridgeProgress) -> bool`. Le callback reçoit phase, nombres
traité/total, objets, octets, durée et processus Git lancés. Retourner `false`
annule à la prochaine frontière de progression ; cela n'interrompt pas une
commande Git déjà en cours. `ExportComplete` est une notification après succès,
non annulable. L'installation SQLite atomique ne fournit pas encore de callback.

Le rapport de scan indique sélection, mapping commits/Steps/références, octets et
avertissements. Branches locales et tags de commits seulement : reflogs,
références distantes et commits accessibles uniquement depuis un HEAD détaché ne
sont pas importés. LFS, gitlinks/submodules, dépôts shallow/partiels, tags
imbriqués/non-commit et chemins non représentables sont refusés explicitement.
Il n'y a pas de synchronisation Git bidirectionnelle continue.

Pour sélectionner des branches/tags précis, utiliser
`scan_git_selected(source, &ImportSelection::References(vec!["refs/heads/main".into()]), limits)`
ou `scan_git_selected_with_progress` avec le callback habituel. Les noms doivent
être des références locales complètes et uniques ; une référence absente fait
échouer tout le scan. Chaque tête conserve tous ses ancêtres : aucune expression
de révision ni sélection de sous-arbre ne tronque les parents. Une branche non
sélectionnée peut contenir des objets incompatibles sans bloquer la sélection ;
les limites globales et le contrôle de stabilité des références restent actifs.
Les APIs `scan_git` existantes continuent de sélectionner toutes les branches
locales et tous les tags de commits. Des sélections successives se chevauchant
ne constituent pas une synchronisation incrémentale : une collision de nom de
Layer/Marker est refusée explicitement.

L'export peut utiliser `ExportMode::SelectedHistory { references: vec![...] }` :
la liste désigne les noms Git de sortie exacts, avec tous les ancêtres de chaque
tête. Les branches importées gardent leur nom Git ; les Layers natives utilisent
`refs/heads/layrs/<nom échappé>` et les Markers natifs `refs/tags/layrs/<nom échappé>`.
Pour une Layer divergente, sélectionner explicitement les noms
`<branche>/head-<hash Step>` voulus ; le nom de base ambigu est refusé.
Les références absentes, dupliquées et les listes vides sont refusées avant
création de destination. Les collisions et conflits restent refusés dans
l'histoire sélectionnée ; aucune coupure d'ancêtre ne les masque.
La sélection normalisée est intégrée au checkpoint v1 existant : mêmes noms
dans un autre ordre acceptés, sélection différente refusée à la reprise.
Le checkpoint complet est plafonné à 16 Kio avant création du dépôt Git.
Cela sélectionne des références et leur fermeture, pas une plage arbitraire
de commits ni des sous-arbres. Le mode `Snapshot` reste le choix explicite
pour exporter un état sans ses parents.

Les collisions de casse dans les Trees sont conservées avec avertissement,
y compris `A`/`a`, préfixes de dossiers, fichier/dossier et Unicode NFC.
L'export bare préserve leurs octets ; un checkout Windows refuse ces chemins
avant matérialisation. Les références Git utilisent aussi des chemins : sur
Windows, des noms tels que `refs/heads/Main` et `refs/heads/main` (ou leurs
préfixes fichier/dossier) sont refusés avant création de la destination.
Une sélection explicite non ambiguë reste possible, sans renommage silencieux.
Ce refus Windows est conservateur, même sur un volume configuré sensible à la casse.

La reprise d'installation réutilise le journal transactionnel du writer natif,
sans second fichier journal modifiable par le pont. Avant commit, des objets CAS
vérifiés peuvent rester sans référence ; aucun Layer/Marker partiel n'est publié.
Après une réponse perdue, le même import vérifie son rapport canonique, son View
historique et ses observations avant de retourner `alreadyImported`. Les têtes
modifiées ultérieurement restent intactes. L'import direct rescane Git ; les
commandes de cache ci-dessous permettent une reprise sans source. Les tests injectent des
erreurs à 15 frontières CAS/SQLite et vérifient le rejeu du lot exact par digest,
le refus d'une identité transactionnelle réutilisée et la conservation intégrale
de la source/du checkout. Ce n'est pas une qualification de coupure électrique.

Pour conserver l'acquisition avant conversion, utiliser un fichier neuf hors
du dépôt source et de son répertoire Git commun :

```text
layrs git scan-cache SOURCE --output CACHE --ref refs/heads/main --json
layrs git import-cache CACHE --digest sha256:DIGEST --dry-run --json
layrs git import-cache CACHE --digest sha256:DIGEST --json
```

Le digest complet renvoyé par `scan-cache` doit être conservé séparément et passé
à chaque reprise. Les APIs correspondantes sont `prepare_git_scan_checkpoint`
et `resume_git_scan_checkpoint`. Le cache contient les octets sensibles de toute
l'histoire sélectionnée : protéger son emplacement et son digest ; permissions
0600 à la création sur Unix, ACL héritées sur Windows. Ce digest vérifie les octets,
il ne signe pas leur provenance. Ne pas placer ce fichier dans les métadonnées
d'un Store natif.

La reprise ne lit plus la source et ne lance aucun processus Git. Elle vérifie
framing/version, tailles, digest externe, identités Git SHA-1/SHA-256, topologie,
fermeture des commits et annotations ; le convertisseur habituel revérifie les
arbres/contenus, chemins et identités natives avant installation. Les références
sont figées et comparées au début/à la fin de l'acquisition ; les changements
ultérieurs de la source nécessitent un nouveau cache. La conversion reprend
depuis le premier commit du cache, pas depuis un état mémoire sérialisé.

L'acquisition doit être complète avant publication : une interruption avant ce
point impose de la recommencer. La publication utilise un fichier exclusif
synchronisé puis un lien physique sans remplacement ; un système de fichiers
sans liens physiques est refusé. Un temporaire `.incomplete` peut subsister après
une erreur, mais n'est jamais repris implicitement. Pas de garantie de durabilité
à la coupure électrique ni de plafond OS global de mémoire ; les octets lus,
objets et en-têtes sont bornés avant leurs allocations.

Limites par défaut : 10 000 commits, 100 000 objets, 100 000 entrées d'arbres,
profondeur 64, 16 Mio par objet et 64 Mio cumulés. La provenance encodée est elle
aussi plafonnée. Les subprocessus Git ont une échéance commune de 60 secondes
pour chaque scan/phase d'écriture d'export ; ce n'est pas une limite OS dure de
mémoire ni un délai global de la transaction SQLite. Le scan précharge les objets
via `cat-file --batch`, vérifie chaque taille/type/identité SHA-1 ou SHA-256 puis
décode les Trees binaires. Les données restent en mémoire dans les plafonds
déclarés. L'export construit un pack Git v2 sans deltas : SHA1 locaux,
arbres binaires triés selon l'ordre Git, compression zlib, checksum global.
Un seul `index-pack --stdin --strict` installe les objets ; son checksum est
comparé au pack préparé avant la publication atomique des références et le
fsck final. Le pont n'écrit jamais directement dans la base d'objets Git.
Les buffers commit/tag/pack sont bornés avant croissance et le pack passe
au thread stdin sans copie supplémentaire. Les plafonds de contenu brut et
de pack encodé sont chacun `max_total_bytes` ; leur mémoire s'ajoute aux
objets natifs chargés et au processus Git, sans garantie de mémoire OS globale.

Validation : `cargo test -p layrs-git-bridge` utilise de vrais dépôts Git locaux
et des bases natives SQLite, vérifie bytes/modes/parents/références/provenance,
reprise, divergences et refus des conflits. La recette ignorée
`volume_and_read_only_pilot_measurements` crée 2 000 commits dans un dossier
temporaire, importe puis réexporte le dépôt. `LAYRS_GIT_PILOT_SOURCE` ajoute un scan
lecture seule du dépôt choisi ; `LAYRS_BRIDGE_MEASURE_PATH` reçoit le rapport JSON.
`LAYRS_BRIDGE_KEEP_FIXTURE=1` conserve explicitement la fixture de cette recette
et affiche son chemin, pour répéter seulement l'export sans réimporter.
Lancer explicitement avec `-- --ignored --nocapture`. Mesures et limites observées
sont consignées dans `docs/plans/pme/journal/T7.md` ; aucun engagement de performance
n'en découle. Le pic mémoire Windows mesure seulement le processus Rust de test,
pas les sous-processus Git ni une limite globale de mémoire.

Références du prototype : [git-cat-file](https://git-scm.com/docs/git-cat-file),
[git-fast-import](https://git-scm.com/docs/git-fast-import),
[pack-format](https://git-scm.com/docs/pack-format),
[git-check-ref-format](https://git-scm.com/docs/git-check-ref-format).
