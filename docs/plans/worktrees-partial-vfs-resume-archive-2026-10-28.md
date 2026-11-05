# Reprise worktrees, projection partielle et VFS — archive (avant le 29 octobre 2026 au soir)

Suite du document courant `worktrees-partial-vfs-resume-2026-10-28.md` : sections les plus anciennes, déplacées telles quelles pour respecter la limite de taille des fichiers.

## Reprise effective du 29 octobre — C13 r101 intégré, compilation en cours

L'utilisateur a demandé « Okay reprends ! » et des revues Opus 5.5 ponctuelles
en parallèle. Goal vérifié **active**. Cette demande lève la pause ci-dessous.

- r100 terminal exit0 reçu : **six PASS**, copie partielle native975,108s.
  Packager exécuté une fois : archive701fichiers/11670323octets, SHA256
  `59c311fd5bbb9f71bc5cbdfc1ddbebc6a7dee6a1cc7efe43617f8b072e781ab5`.
  Archive/inventaire sous docs/continuation/worktrees-partial-vfs. Aucun rerun.
- Proposition C13 interrompue reprise par ROOT : snapshot de nuit intégralement
  conforme, 79 chemins (78 Rust), 49 modifiés depuis la proposition source-B53.
  Tous les chemins des53 conservés ; rustfmt --check et git apply --check PASS.
  Gel `tmp/c13-complete-lease-ready-source-proposal/manifest-r101.json`, SHA256
  `b806f17060b0b252be720490c16c5b3caba5f45d2ed2881e0b931f2f1df667f2`.
  Patch exact HEAD `proposal-r101.patch`. Ces sources sont maintenant intégrées
  au dépôt principal, en plus des cinq fichiers de tests r100. Les fichiers du
  bundle restent immuables pour la revue ; corrections ROOT dans le dépôt puis
  nouveau gel ultérieur. Ne pas relancer le script d'intégration.
- `py D:/Layrs/tmp/integrate-and-build-c13-r101.py` lancé UNE fois :
  **ROOT10981** compilation store-lib --no-run en cours, suivie du workspace
  all-targets seulement si elle réussit. Rapport
  `tmp/c13-complete-ready-build-results-r101.json` et logs voisins.
  Aucun runtime C13 exécuté. Aucun autre Cargo/SQL/natif simultané.
- Opus5.5 disponible via `claudeAgent`, modèle `claude-opus-5-5` ; revue
  indépendante ciblée sur le nouveau raccordement, le maintien des pins et
  les erreurs/ACK. Préférence explicite utilisateur pour ce modèle de revue.
- C14 stockage : brouillon de nuit conservé, toujours non intégré/non compilé.
  C13 ne qualifie pas C14, l'hôte client/CLI, la reprise avec nouvel owner ni
  le réexport sortant Ordinary9/10 des compagnons. Programme L0–L9 reste actif.

Suite compilation : ROOT10981 terminal exit1, Cargo101 après28,018s ; quatre
erreurs conservées dans le log r101. ROOT a corrigé export read_ancestry,
conversion CauseResult, clone du chemin de test emprunté et dispatch strict/
publication dans ProviderContext (pas de faux proof pour le parcours Strict).
Gel intégré suivant `tmp/c13-complete-ready-integrated-r102/manifest.json`,
79 fichiers ; SHA256 `ae0ef70270a1130bd1e3556b58cc8ecf79f1f42e367feb4bafff695f5a93aa67`.
Exactement quatre fichiers changés depuis r101 ; rustfmt --check PASS.
`py D:/Layrs/tmp/freeze-and-build-c13-r102.py` lancé une fois, **ROOT49288**.
Rapport `tmp/c13-complete-ready-build-results-r102.json` ; compiler store-lib puis
workspace all-targets, aucun test. Revue Opus5.5 doit porter sur ce gel immuable,
et utiliser HEAD comme contexte pour les fichiers absents, pas sur un ancien lot.

## Arrêt demandé pour la nuit — 29 octobre 2026

L'utilisateur a explicitement demandé : « Alright arretons nous pour ce soir ».
Mettre le goal en **pause** après sauvegarde ; NE PAS reprendre sur notification
de fin d'un test ou d'un ancien sous-agent. Attendre une demande de reprise.
Aucun push/PR ; HEAD reste `6b1f5632bd7f6092226d57e3c813033a82bcfc98`.

- **r100 : cinq SQL PASS** (21,191 / 16,336 / 87,936 / 54,274 / 43,674 secondes).
  Le sixième et dernier scénario natif de copie partielle est encore en cours,
  handle ROOT25207. Il est laissé finir seul et enregistrer son résultat : aucun
  test supplémentaire ne suit. Ne pas le relancer ni rouvrir ses fixtures.
  Résultats : `.layrs-local/validation/provider-ordinary-checkout-boundary-partial-20261028-r100/ordinary-checkout-boundary-partial-results.json`.
  Aucun terminal observé à l'arrêt ; packager r100 NON exécuté. À la reprise,
  récupérer le terminal/logs, puis archiver succès OU échec sans masquer l'état.
- **C13 : arrêt confirmé** (`interrupted`, aucun run actif, 00:01:08 UTC)
  après task_cancel pour la tâche
  `33e27463-7cc9-443b-b7c9-d266c63aeccc` (lease complet et vrai C Ready).
  Ses fichiers partiels restent dans `tmp/c13-complete-lease-ready-source-proposal/`.
  Aucun manifeste final/patch final ni compilation/acceptation revendiqué.
  Snapshot de 103 fichiers, relu dans l'archive et sans dérive pendant sauvegarde :
  `docs/continuation/worktrees-partial-vfs/c13-lease-ready-interrupted-draft-20261029.tar.gz`,
  SHA256 `9bc56fdc450a4043101fdbbca2b1097454d57add6982334bfc7c127a7fff7596`.
  Reprendre cette proposition existante, ne pas redémarrer toute l'implémentation.
  Vérifier ses changements et l'intégration des trois correctifs ROOT avant build.
- **C14 : brouillon SQL de stockage écrit, NON formaté/compilé/exécuté** dans
  `tmp/provider-denied-coverage-storage-proposal/` : six Rust (trois records déjà
  figés + module provider, stockage exact et quatre recettes SQL nouvelles).
  Transaction appelante obligatoire, FK pour insertion, taille lue avant BLOB,
  validation corps/référence, premier INSERT et replay exact sans réparation.
  Aucun raccordement Planned-v2, receipt-v2, prefix-v3, copie, preuve live ou
  fence d'acceptation encore effectué. Les quatre recettes sont ignorées et NON
  exécutées ; ce module reste une proposition DATA, pas C14 terminé.
  Snapshot brut sept fichiers avec base-inputs, sans dérive :
  `docs/continuation/worktrees-partial-vfs/provider-denied-storage-unverified-draft-20261029.tar.gz`,
  SHA256 `9781a027671ed4921adfa1f84b933bb7ff43f726b5d44c0d1d2a2e74a2857a53`.
  À la reprise : relire le diff, formater/vérifier, finir les liens protocolaires
  sur la base cumulative C13 avant qualification. Ne pas traiter l'archive brute
  comme une release source validée.
- Inventaires et checkpoint détaillé :
  `docs/continuation/worktrees-partial-vfs/night-stop-checkpoint-20261029.json`.
  Les modifications du dépôt principal restent cinq fichiers de tests r100 et
  les documents/archives. Aucune production Provider proposée n'est intégrée.

La priorité suivante reste le parcours C13 complet compilé et testé, puis C14
et le reste du programme L0–L9. Les anciens lots acquis ne sont pas à rejouer.

## Dernier checkpoint — checkout r98 cinq PASS ; r99 build/check PASS ; r100 en cours

Checkpoint local **6b1f5632bd7f6092226d57e3c813033a82bcfc98**, 23 fichiers,
stash original préservé ; aucun push/PR. Goal confirmé **active**.

- **r91 : 7/7 PASS**, archive déjà vérifiée et commitée. Ne pas relancer.
- **ROOT86893/r94 terminal exit0 : 5/5 PASS** : SQL44,665s, fresh575,541s,
  DeclareACK576,062s, OwnedACK577,891s, retained652,120s.
  Packager r94 exécuté UNE fois : archive708fichiers/7162733octets, SHA256
  `ac0284032d0ca7dfd16face21f009716d37f9e3a2d8c92dfc921f23a209eaa94`.
  Archive et inventaire sous docs/continuation/worktrees-partial-vfs restent
  à commiter. Ne pas réexécuter runner/packager ni rouvrir les fixtures.
- Revues r92/r95 et corrections ROOT r93/r93b/r96 conservées. r95 relève deux
  défauts de témoins corrigés : cause structurée des refus allocation-gap et
  garde zéro-writer autour de la réouverture elle-même. Pas de nouvelle revue
  pour ces petits correctifs. Les cinq nouveaux tests sont commités.
- **r97 build et workspace/all-targets PASS**, logs terminaux : 1m01s/21,19s.
  Ancien handle ROOT59569 désormais absent ; aucun rebuild nécessaire.
- **r98 release exécutée UNE fois**, SHA256
  `a45d7641add7813d056fcd218391a57d704ecb9cedaa89be8a81cefb97a6dc89`.
  672 sources retenues ; seul fichier de tests append-only différent de r94.
  EXE82693632octets SHA256
  `5c5c1adf1b14c91c8fc366947840385fcc265cf18f52e6b05d62cf0469176e57`,
  CodeView621a98b1-e6bc-410a-9a0f-44ddcc208f5e age140.
  **ROOT99703 terminal exit0**, runner provider-ordinary-checkout-late-run-r98.py r98,
  lancé une fois avec accès Windows autorisé. Cinq nouveaux natifs uniquement :
  Verified ACK, Publishing ACK, rename-gap, stage allocation-gap, file allocation-gap.
  Verified ACK **PASS1206,615s**, Publishing ACK **PASS1194,779s** ;
  rename-gap **PASS1258,106s**, stage allocation-gap **PASS1118,518s**.
  file allocation-gap **PASS1149,415s** ; cinq PASS. Terminal observé le28/10
  23:43UTC (29/10 local). Ne pas rejouer les cinq scénarios acquis.
  Préparation première recette1046,296s ; PID enfant72876 observé vivant avec
  CPU917,234s durant cette préparation. Aucun blocage/timeout diagnostiqué.
  Aucun Cargo/build/SQL/natif concurrent. Les deux allocation-gap qualifient
  un refus préservant les objets, PAS une adoption/réparation réussie.
  Packager r98 exécuté UNE fois après terminal : archive702fichiers/9605282octets,
  SHA256 `41cb195a597cbb63eb071fe1e70730055fcc8f64bb77657bac34c9cbbd2c83c3`.
  Archive et inventaire sous docs/continuation/worktrees-partial-vfs, à commiter.
  Lecture seule des trois logs PASS → tmp/checkout-r98-first-three-phase-diagnostics.json.
  Coût principalement setup : 1030–1079s, dont installation des trois manifests
  440–468s et construction Provider→B admis 300–315s. Pas de diagnostic OS/lock,
  ni mesure comparative contrôlée : phases imbriquées non additionnables.
  Packager r98 retient cette analyse après vérification de ses hashes de logs ;
  AST et rétention PASS. Ne pas rouvrir les fixtures ni rejouer ces trois.
- Proposition SQL de borne C10 livrée par ordinary_checkout_progress_sql sous
  tmp/ordinary-checkout-c10-floor-proposal/ : T−1/T, construction avant effets,
  contrôle positif Ready8 puis rollback. Candidate SHA256
  `a6c069075b8ff114a87b8bc4545f24e87bff49170f0e98c49528ad4d97fb8f22`.
  Intégrée par ROOT après lecture du diff, vérification des bornes indépendantes,
  du contrôle Ready8 et du tail de négatifs/rejeux inchangé. Rustfmt/check PASS.
  root-integration.json conserve hashes et acceptation SOURCE ONLY. Lors de cette
  intégration, seul ce fichier de tests différait des 672 entrées retenues r98.
  Non compilée, non exécutée. Après ROOT99703 terminal, compiler puis exécuter
  le nouveau SQL et les quatre anciens SQL utilisant le setup refactoré.
  Ne pas relancer les natifs inchangés. Ne pas confondre cette source avec r98.
- Delta suivant ajouté par ROOT : test natif de véritable écriture partielle,
  `managed_ordinary_fresh_checkout_partial_write_retains_owned_inode_and_restarts_copy`.
  Hook local au thread sous cfg(test), dans le sink du driver Owned : Write::write_all
  écrit réellement la moitié de CURRENT puis reçoit une erreur I/O typée. Aucun
  TEMP fabriqué/modifié manuellement. Témoins : trois closes SQLite acquittées,
  journal File Owned, préfixe réel non vide, pin conservé puis libéré, réouverture
  sans writer ni réparation, réécriture depuis zéro, même inode publié et replay.
  Aucune panne OS/crash/perte d'owner n'est revendiquée ; petit fichier courant.
  Rustfmt/check cinq fichiers PASS ; compilation et runtime NON exécutés.
  Gel tmp/ordinary-checkout-boundary-partial-source/ : quatre anciens fichiers
  modifiés + un nouveau module test, manifest SHA256
  `fe1c0755b1252e8279a909147bbd00f08b4b73296bda9625ccc4224d0a097f79`.
  Les anciennes recettes fresh checkout depuis fn snapshot restent identiques
  après normalisation LF. Revue native indépendante checkout_partial_copy_review
  terminée, SOURCE ONLY, sans défaut actionnable ; cinq hashes début/fin conformes.
  Rapport entièrement lu :
  tmp/provider-ordinary-checkout-boundary-partial-source-review.json.
  Après r98 terminal et revue traitée : build/check puis cinq SQL (nouvelle borne
  et quatre anciens callers au setup modifié) et ce seul nouveau natif.
  Scripts préparés, AST PASS :
  `py D:/Layrs/tmp/build-checkout-boundary-partial-r99.py --r98-terminal-observed`
  puis `py D:/Layrs/tmp/prepare-and-release-checkout-boundary-partial-r100.py --root-accept-source-review`.
  R99 lancé UNE fois après r98 terminal+archive, **ROOT25872 terminal exit0** :
  build/store-lib-no-run PASS52,85s puis workspace/all-targets PASS24,48s. Aucun test
  dans cette commande. Préparation/release r100 exécutée UNE fois, **ROOT93289 terminal0**.
  R100 retient673sources, cinq fichiers revus différents de r98, six recettes.
  Release SHA256 `539e46b3885271ee6509313f6ec30baac1871e5228b8f995c2d2193405db11dd` ;
  EXE82814464octets SHA256 `e0a903f03f7907b416a1932f1a047c72d8716c0fae866590b98ab89369025495`,
  CodeView621a98b1-e6bc-410a-9a0f-44ddcc208f5e age141.
  Runner `py D:/Layrs/tmp/provider-ordinary-checkout-boundary-partial-run-r100.py r100`
  lancé UNE fois avec accès Windows autorisé, **ROOT25207 actif** : borne C10
  PASS21,191s, private Ready SQL PASS16,336s, family publication SQL démarrée.
  Aucun autre Cargo/SQL/natif simultané ; packager r100
  NON exécuté, à lancer seulement après observation du terminal. Ne pas relancer.
  Ils exigent r98 cinq PASS, sources figées conformes et revue sans finding ;
  la rétention r100 exige aussi archive r98 et build/check r99 terminaux PASS.
  R100 retiendra 673 sources et six recettes. Aucun test C13 dans ce lot.
  Si r98 échoue, conserver l'échec, ne pas contourner ces assertions.
  Packager `package-checkout-boundary-partial-r100.py` également préparé,
  AST PASS, NON exécuté ; exige résultat r100 terminal avant usage.
- Proposition C13 T3 **terminée**, thread lu jusqu'à position94/hasMorefalse.
  Bundle tmp/c13-companions-source-proposal/ : 17 fichiers, 16 Rust, rustfmt PASS.
  Manifest SHA256 `62b21e4638d7d1aa42a51f67c13387b56eb9383285c5ff6403cf4771552844aa`.
  Dépôt principal inchangé. Codec v2, owner dérivé du reconcile, image complète,
  plan DATA et primitives SQL proposés. **U1–U7 non achevés** : raccordement actif,
  installation/protection, publications, Ready, réexport9/10, autonomie, qualification.
  Les vecteurs d'intentions/représentations dans l'en-tête64Kio sont une limite
  fonctionnelle à garder explicite ; aucun débit/quota augmenté implicitement.
  Ne pas accepter ce bundle comme C13 terminé ou comme preuve runtime.
  Proposition d'origine archivée sans intégration :
  c13-companions-source-proposal-20261028.tar.gz,115471octets,25entrées,
  SHA256 `b276be0f3485597acb46f38b99c50f2bb2f4b08f16addd20373e92973e6a744a`.
  Archive et inventaire dans docs/continuation/worktrees-partial-vfs, à commiter.

Prochaine tranche : raccordement C13 au parcours Provider réel et préservation
avant Ready ; conserver les garanties Ordinary existantes. Les tests r98 acquis
ne seront pas répétés sans changement pertinent. Le reste du programme L0–L9
reste à poursuivre ; aucune réussite de lot ne clôt le goal complet.

Suite C13 terminée via T3, GPT-6.1-Sol/high, shared avec dépôt READ ONLY,
sorties uniquement tmp/c13-provider-driver-source-proposal/ :
taskId node:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A9c871aa5-1484-4824-a6d5-65b0d88f354f.
threadId thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A9c871aa5-1484-4824-a6d5-65b0d88f354f.
Mission : U1–U4 et autonomie Provider-produced U6, test intégré réel ; réexport
Ordinary9/10 U5 explicitement différé. Limite v2 64Kio conservée dans cette tranche,
avec refus précoce testé et limitation documentée ; aucune refonte DDL en parallèle.
Premier appel cellule803 refusé AVANT création child : option `effort` inconnue.
Capabilities reconsultées ; nouvel appel utilise `reasoningEffort: high`, accepté.
Statut completed, thread entièrement lu jusqu'à position79/hasMorefalse.
Livraison encore **partielle, driver non implémenté** : 23 fichiers cumulés,
manifest SHA256 `0033a04e8831186c79b0d3b8581e1d36d1f5083a56470194c0a672ff4186835c`,
patch SHA256 `fd41a6a42d91e0ad4ebdf702421ee3716c9a327a65d96af53803516efc28e1ba`.
Lecture héritée v2 et union exacte raccordées dans la proposition ; deux recettes
non exécutées ajoutées. Rustfmt/check22Rust PASS. Pas d'intégration ni compilation.
Blocages CONCRETS : aucun caller produit Provider8 de conversion ; aucune
observation d'autorité couvrant la sélection entière acquise côté client ;
protections/mappings source avant FK du reçu et nouveau Ready restent à raccorder.
Voir C13-PROVIDER-DRIVER-HANDOFF.md dans ce nouveau bundle. Ne pas redéléguer la
même mission sans traiter ces dépendances ; aucun Cargo/build/SQL/natif exécuté.
Bundle cumulatif retenu sans intégration :
`c13-provider-driver-source-proposal-20261028.tar.gz`, 149566 octets, 32 entrées,
SHA256 `9fe82ed476e8eab82b7c9ef1ca49755024c6aad58d010ea8f35e894e98074ae4`.
Les 23 sorties et les bases Git ont été vérifiées avant archivage ; les 31 fichiers
du bundle ont été relus dans l'archive. Inventaire adjacent, à commiter. Premier
essai d'archivage arrêté avant toute écriture sur un décompte erroné des fichiers
annexes (8, pas 7), puis corrigé. Aucune qualification runtime C13.

Investigation source C13 également terminée et lue entièrement par ROOT :
`docs/continuation/worktrees-partial-vfs/c13-source-ownership-design-20261028.json`.
Décision d'architecture pour l'implémentation suivante, NON implémentée/qualifiée :
réservation fixe et racine exacte de rétention appartenant à B ; C reste seul
installateur et propriétaire des débits. Aucun emprunt au budget Capture ou C,
aucune augmentation du quota. Les validateurs doivent reconnaître explicitement
ce nouveau groupe et conserver les contrôles de complétude et les fences GC.
Après C privé ET famille Ready, insérer atomiquement dans B les mappings exacts,
compagnons, claim, reçu et passage Staging→Ready ; puis fermeture reconnue,
relecture et passage Ready→Published séparé. Le préfixe historique reste inchangé.
Cela corrige le contrat temporel de la proposition non intégrée : protéger B et C
avant l'un ou l'autre reçu, mais persister les lignes de chaque base avant SON
reçu. Les deux bases n'ont pas besoin de leurs lignes avant le reçu de C.
Le prochain diff devra modifier explicitement ce texte de proposition et tester
les états B réservé seul, B protégé/C non Ready, C Ready/B Staging et B Ready.
Pas de changement DDL nécessaire identifié ; les mappings limités à l'en-tête
ne respectent pas les contrats v2 actuels. L'exemple de formule Q_B dans le
rapport n'est PAS une formule acceptée : son calcul déterministe, ses bornes et
son coût doivent être établis avant les effets et vérifiés dans le prochain lot.
Ne pas refaire cette investigation ni la revue des cinq sources test inchangées.

Investigation C14 ciblée terminée par programme_remaining_inventory :
tmp/c14-authority-integration-map.json, SHA256
`c5872ab4271f7b78909944c6e6c7146edc1e481f0ad31a3a03334257041b04d8`.
Chemin existant : NativeRemote bootstrap → projection_page complète →
projection_access pour tous anciens/nouveaux chemins et absences → manifest
metadata seulement → fence finale. fetch_plan ne compare pas automatiquement
l'epoch de Binding.observed ; contrôle explicite requis. Aucun lease serveur
persistent ; HTTP avant gates Store puis revalidation de l'ancrage source.

Implémentation de cette dépendance client terminée, GPT-6.1-Sol/high, T3 async :
taskId node:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A1deb3b3b-eba7-437b-9940-6c5d91845df4.
threadId thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A1deb3b3b-eba7-437b-9940-6c5d91845df4.
Sorties uniquement tmp/provider-transition-observation-source-proposal/ ;
ownership client-core remote provider_observation + tests + wiring/helper étroit.
Dépôt READ ONLY, aucun Cargo/build/test/SQL/natif/network/fixture/process polling.
Collecteur réel avec constructeur privé, deux stratégies d'epoch, absence refusée
séparée, aucun téléchargement de chunks, aucune mutation Store/remount, tests HTTP
écrits mais non exécutés. Notification reçue ; thread lu jusqu'à position62,
hasMorefalse. Bundle trois Rust : API réelle observe_provider_transition et
revalidate_provider_transition_scope, 21 tests HTTPS écrits, rustfmt/check et
rejeu mémoire du patch PASS ; aucun build/runtime. Handoff entièrement lu.
Patch SHA256 `cba3ebbeaa62a7707ef7482a2e68fcb668146fdd81b4b05b7ede36d9a6eaeb85`.
Revue critique native indépendante `provider_observation_critical_review` terminée,
SOURCE ONLY, sortie tmp/provider-transition-observation-critical-review.json.
Un P1 de compilation POBS-001 : match_indices produit (usize,&str), sentinel char
incompatible. Deux notes de couverture, pas de contournement d'autorisation
concret trouvé. Empreintes début/fin et base conformes ; aucun runtime.
ROOT a traité le petit correctif sans redélégation dans une candidate distincte
tmp/provider-transition-observation-corrected-proposal/ : seul changement produit
est le sentinel string "/". Tests renforcés : 100001 absences triées uniques sans
HTTP, renouvellements cohérents modifiés (descripteurs/digest/ranges) passant les
validateurs de plan avant refus du collecteur ; bearer vérifié, digest de fixture
opaque distinct du wire ; nom du test cache limité à l'epoch réellement testée.
21 fonctions, cas supplémentaires non exécutés. Rustfmt/check et Git apply --check
PASS ; compilation/runtime restent requis. Original et revue inchangés conservés
sous reviewed-input. Manifest corrigé SHA256
`35bae32f11b6d3f31d19d340c886b984484146a31259b6e1522203e5d68b0861` ; patch
`8a409e6ce5feae17449d2b623feda66fae44719d536a806b4f51ad28852c7121`.
Candidate corrigée archivée et relue une fois, 14 entrées/82456 octets :
provider-observation-corrected-proposal-20261029.tar.gz, SHA256
`f6517656cc73d738bcf290c9e4833465d70b3c49c5266bba2ce3d494b4234d7c`.
Inventaire adjacent, à commiter. Original et revue inclus ; aucune qualification.
Ne pas réappliquer l'ancienne candidate ni refaire sa revue inchangée. Pas de
build/test concurrent avec r98 ; préserver le delta r99/r100 déjà figé avant
intégration des propositions Provider/client.

Implémentation source B terminée via T3 GPT-6.1-Sol/high, dépôt READ ONLY :
taskId node:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A4d8663ee-8893-4192-ba0f-08c0ec276920.
threadId thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A4d8663ee-8893-4192-ba0f-08c0ec276920.
Mission exacte tmp/c13-source-protocol-implementation-task.txt ; sorties uniquement
tmp/c13-source-protection-source-proposal/. Raccordement réel réservation/racine B,
preuve privée/famille complète, financement et publication atomique source,
préservation du préfixe et des gardes C Ready. Pas de Cargo/build/test/SQL/natif.
Notification reçue, thread lu jusqu'à position113/hasMorefalse. Bundle cumulatif
53 fichiers dont 52 Rust ; original23 conservés, rustfmt/check PASS, aucun build.
Manifest SHA256 `601c7e851214849c418c04d9852c8ccd1622243c8899a921e9e9c93aa3516c69` ;
patch SHA256 `c1e53502d0691c1d27727435c555f0d5a70fece7f6f33aab20bbe3b098290987`.
Handoff lu entièrement : validateurs privés/famille et modes complets, réserve
B distincte et budget exact R19 sans plancher16MiB, protection CAS/image, writer
source atomique et scans autonomes raccordés. Les constructors C Ready et le
lease legacy gardent leurs refus de nouveaux compagnons ; conversion non finie.
Reste précisément le lease conservant l'image complète/reconcile et le Ready
compagnon réel ; pas de permission pour contourner les anciens guards.
Bundle53 retenu et relu UNE fois :
`c13-source-protection-source-proposal-20261029.tar.gz`, 312784 octets, 74 entrées,
SHA256 `078ebf7c45cb0f55a823b14b36b026942238828dc86600637360b2695d8eb5f8`.
Inventaire adjacent ; toutes les 53 sorties et bases Git vérifiées. Rétention
de proposition, sans acceptation compilation/runtime. Handoff JSON et recettes
également lus entièrement ; trois tests réels écrits, autres recettes prospectives.
Deux missions distinctes préparées pour T3 background, sorties sans conflit :
`tmp/c13-source-protection-critical-review-task.txt` (revue Astra/xhigh du lot53,
sortie `tmp/c13-source-protection-critical-review.json`) et
`tmp/c13-complete-lease-ready-implementation-task.txt` (Sol6.1/high, raccordement
concret image complète/lease/Ready/publis, sortie cumulative
`tmp/c13-complete-lease-ready-source-proposal/`). Cette dernière doit joindre les
appels de production et écrire une recette intégrée, pas livrer seulement des
interfaces. Elle conserve les 53 sources et fusionne la vue locale neuf fichiers.
Client host complet, denied-absence durable, reprise avec nouvel owner et réexport
Ordinary9/10 restent explicites hors tranche. Aucun Cargo/test concurrent autorisé.
État exact des délégations dans leurs résultats T3 ; ne pas les relancer sur
la seule base de ce paragraphe préparatoire. Goal confirmé active au 29/10 local.
Les deux délégations ont été acceptées et lancées :
revue taskId node:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3Afcb60a92-64ce-431e-87f8-1e3dc34fbec3 ;
implémentation taskId node:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A33e27463-7cc9-443b-b7c9-d266c63aeccc.
Les threadId correspondants remplacent le préfixe node: par thread:.
Revue fcb60 terminée, thread lu jusqu'à position114/hasMorefalse ; implémentation
33e274 encore active (steering confirmé), aucun nouveau lancement ni polling.
Rapport retenu dans docs/continuation/worktrees-partial-vfs/c13-source-protection-critical-review-20261029.json.
Trois findings concrets : CR001 P1 snapshot FS après ACK refusé, CR002 P1 INSERT
suppression8valeurs/7colonnes, CR003 P2 collection4096racines pouvant retenir64GiB.
ROOT a corrigé dans `tmp/c13-source-protection-review-fixes/`, sept fichiers en
overlay du lot53, original inchangé : reader strict, sept placeholders, parcours
streaming avec callback de validation avant racine suivante et seuls IDs conservés.
Les trois callers sont raccordés ; deux recettes SQL nouvelles écrites (première
insertion/replay/conflit et streaming/refus avant payload suivant), NON exécutées.
Rustfmt/check et apply --check sur le lot53 PASS. Manifest SHA256
`b1d05862814f29e011b6c64e126bd4cb2096efd9da1af80cb139ad5ce26a8fcd` ; patch
`943972e50f11ab092d29b16fbc8bc9d9f781e1d42d174b2f76464365d03d9413`.
Correction transmise par steering à33e274 pour fusion cumulative ; pas de nouvelle
revue déléguée pour ces corrections. Compilation/runtime et validation intégrée
restent à faire. W3/W4 doivent parler de close SQLite, pas de close OS observée.

ROOT a corrigé une dépendance réelle distincte dans
`tmp/provider-observation-prefix-domain-fix/` : le normaliseur calculait toujours
source_prefix_id avec le domaine v1, même pour les préfixes v2 valides ; il appelle
désormais prefix.digest(), versionné. Deux fichiers, une régression DATA v1/v2
via le normaliseur réel ; v1 reste inchangé. Rustfmt/check et apply --check PASS,
aucune compilation/exécution. Manifest SHA256
`a6ece02687428485849bb451c51a14e128b3c11f96ccf1d6258a228b41f5ef43` ; patch
`e0257ead1139be9a9fc0366e090a3a2140a8f05839ac860911019793af7db340`.
Petit correctif relu par ROOT sans nouvelle revue déléguée. L'implémenteur lease
reçoit ces deux fichiers par steering pour fusion cumulative ; dépôt principal
et gel r99/r100 restent inchangés. Ne pas considérer ce fix comme C14 durable.

ROOT a écrit une proposition distincte de vue locale :
tmp/provider-transition-source-anchor-proposal/, neuf fichiers dont deux nouveaux.
API publique ProviderActiveSession::transition_source(cancel) →
ProviderTransitionSource<'session>, puis revalidate(cancel). La vue emprunte la
session et conserve la preuve complète, relâche les gates avant HTTP, refuse
capture/transition pendante et compare l'état complet après nouvelle lecture.
Ne fabrique ni prefix_id ni image_digest : le futur driver doit encore les
tirer de la closure/checkpoint réelle et les revalider. Aucun grant writer.
Helper from_publication factorisé dans PrivateCaptureMutationValidation ; le
délégué source B a reçu les interfaces et doit préserver le nouveau companion
proof dans read_live/require_same_state. Fusion étroite de validation.rs à faire.
Manifest SHA256 `b715fce9b14728026def9cf64fcbe81fb237d80b2a8fcc01611c3ebcd315e7bf` ;
patch SHA256 `fd1a62a97267204fd1001245418133eddc08f03f1f00e14f5a944b8fd56e257a`.
Huit Rust rustfmt/check PASS ; projection.rs garde son format historique avec
un réexport ajouté. Git apply --check PASS après correction des headers new-file
du générateur. Aucune application au dépôt, compilation/exécution/revue indépendante.
Recette native écrite : lecture sans writer/hydratation, déconnexion/annulation,
vraie Capture intervenante → ancienne vue refusée/nouvelle acceptée ; la preuve
d'image reste séparée. Child3600/outer3660 proposés non mesurés. Voir HANDOFF.md.
Archive de cette proposition locale créée et relue UNE fois, 14 entrées/38261 octets :
provider-transition-source-anchor-proposal-20261028.tar.gz, SHA256
`3143877c4671fa8d7ce98cb76fd77620a0b3a30648601b5738a00a4bd313913b`.
Inventaire adjacent, à commiter. Rétention de source partielle, aucune acceptation.
Revue native indépendante de cette vue locale terminée sous
`provider_transition_source_review`, SOURCE ONLY, aucun finding. Rapport lu et
conservé sous docs/continuation/worktrees-partial-vfs/provider-transition-source-anchor-review-20261029.json.
Ne remplace pas la revue critique du driver complet après jonction des capacités.
Investigation bornée C14 terminée par `programme_remaining_inventory` : stockage durable
exact des denied-absence paths, contraintes codec/payload/scans/replay/autonomie,
sans se limiter à un hash ou à un refus permanent. Sortie attendue
tmp/c14-durable-denial-contract.json, lu entièrement, retenu sous
docs/continuation/worktrees-partial-vfs/c14-durable-denial-contract-20261029.json.
ROOT retient l'architecture suivante, protocole durable NON encore implémenté : document canonique complet
dans projection_observations existante, liens compacts obligatoires intent/receipt
v2 et rowset import-prefix v3. Les limites 100k/8MiB restent partagées ; document
<=64MiB, plans/SourcePublished<=256KiB et receipts<=64KiB. Pas de nouveau DDL ni
champ optionnel silencieux v1, pas de cycle document→plan→document. Le document
doit être copié/authentifié et rester un fence après admission jusqu'à observation
explicite suivante ; old offline Observation conserve son lecteur strict séparé.
Ce travail n'est pas clos par un simple refus, ni par un digest sans les chemins.

Première couche C14 écrite par ROOT dans `tmp/provider-denied-coverage-record-proposal/` :
trois Rust (module + codec + sept tests DATA), `TransitionDeniedCoverageRecord`
dérivé du normaliseur réel, corps complet canonique, référence compacte, contrôle
des ancrages/version/image/autorité, lecture corps+clé+ref, chemins/alias/bornes
avant copie et désérialisation bornée. 100k chemins/8MiB partagés avec namespace
visible, document64MiB ; aucun grant, writer, version de plan/reçu ou fence actif.
Tests de pleine capacité, tamper, canonicalité, normaliseur et annulation écrits
NON exécutés. Rustfmt/check et apply --check PASS ; main et tous autres bundles
inchangés. Manifest SHA256 `8e21ffab7ab08be56d3108a99dede4529387b4f59b9c859e4f56f07369fd0bfd`,
patch `cc7f278c561ee72053aa3b1a89fdbde21bbe107ac070cc191270b3547ef44022`.
Archive relue UNE fois, 8entrées/19982octets :
provider-denied-coverage-record-proposal-20261029.tar.gz, SHA256
`a98e2cfa0e6373bd7e5a171518eba75f47032a91e20aa8bf62a6f0b5379f2999`.
Inventaire adjacent ; reste toute intégration v2/v3, Planned+document atomique,
SQL scoped, copie, Ready/admission/réexport et fences courantes. Ne pas annoncer
la persistance achevée et ne pas élargir le chantier C13 en cours à cette couche.

Goal **actif**, programme complet ; aucun push/PR. Base des revues `e2a1ac4`.
r66 admission fraîche et r69 publication fraîche : cinq PASS chacun, archives
vérifiées. Reprise fraîche Staging : revue r70 sans défaut, build/check r71 PASS.
Runner r72 **ROOT17382** terminé exit1 : cinq régressions PASS, premier natif
FAIL101/Windows0xc00000fd après642,474s (stack overflow), second non atteint.
Archive vérifiée. Cet échec est désormais suivi des deux succès natifs r79.

Revues **r73/r74 terminées sans finding**, threads lus au résultat final.
Garde Ready et recette C10→D10 compilées r75 ; instrumentation de diagnostic
Staging uniquement test compilée/check r76. r77 terminé : SQL Ready PASS,
natif FAIL ; traces localisent le débordement à open/validated-read puis
validate/observation. Correction privée Box<Validated> compilée/check r78,
réduction de pile compilée mesurée. Runner **ROOT22841/r79 terminé exit0**,
deux natifs Staging PASS599,491s et598,976s, pile inchangée ; archive vérifiée.
Réexport C10→D10 : r80 refus sandbox avant fixture ; r81 FAIL1011189,026s sur
un témoin de budget trop précoce, avant admission D. Archive conservée. ROOT a
corrigé le témoin avec comparaison exacte du nettoyage autorisé source/famille.
Lot familial revu r82 (remarque P3 corrigée), build/check r83 PASS. **ROOT73332/r84
terminé exit0**, sept PASS : quatre courts, réexport1309,558s, famille609,067s et
famille ACK600,052s. Archive vérifiée. Le passage privé frais et les corrections
Ready retenues passent les revues r85/r86. Compilation r87 échoue sur un motif
de test incomplet ; correction ROOT d'une ligne, build37,27s et check14,40s r87b
PASS. **ROOT38516/r88 terminal exit1** : quatre SQL PASS, premier natif échoue
dans le témoin des reçus hérités, avant le driver privé frais ; quatre natifs
non atteints. Archive conservée, témoin corrigé en source. Lot r89 revu sans finding,
build/check r90 PASS. ROOT75810/r91 actif : deux SQL et deux natifs privés frais PASS,
retained privé ACK PASS559,386s, famille Updated en cours. Checkout r92 revu,
R92-01 corrigé en source par ROOT ; compilation en attente. Ne relancer
aucun runner/script déjà exécuté ; les sections finales donnent l'état exact.
Les sections finales donnent hashes, chemins, périmètres et scripts déjà exécutés.
Le reste de ce document conserve la chronologie des preuves, y compris échecs.

L'utilisateur a explicitement demandé « reprends ! /goal resume ». La consigne
d'arrêt nocturne est levée. Le programme complet reste à poursuivre, sans push
ni PR. À la reprise, `get_goal` affiche encore `paused` ; l'API disponible ne
permet pas de réactiver ce statut. Cela ne retire pas l'autorisation de travailler.
Mise à jour : après la nouvelle demande explicite « resume (sans arrêter le goal) »,
`get_goal` confirme maintenant **active**. Le périmètre reste le programme complet.

## Ready et publication familiale

Baseline sauvegardée : `3291ab5` (Ready WIP), `9ab84e0` (famille WIP).

- Échec Ready r44 confirmé dans le témoin de test : il omettait la base
  sélectionnée, absente de l'historique copié mais membre de l'union protégée.
  `ReadyContent` fournit désormais explicitement base/historique/courant. Le
  témoin vérifie l'union exacte, le préfixe historique seul et l'ajout exact
  base+courant. Toutes les comparaisons complètes de tables sont conservées.
- La recette familiale possède deux scénarios sur fixtures distinctes : ACK
  retenu après fermeture réussie, et succès `Updated` avec relecture fraîche,
  chacun suivi de `AlreadyReady` sans writer. Ce second scénario couvre
  l'observation E1 de la revue r45, sous réserve de réussite à l'exécution.
- Build libtest r46 réussi (50,32 s), `cargo check --workspace --all-targets`
  réussi (29,73 s). Les avertissements existants restent présents.
- r46 : 3 tests purs PASS, premier SQL familial FAIL lors du nettoyage.
  Le corps avait déjà fermé la transaction ; la fixture extérieure exécutait
  un second ROLLBACK. Correction des deux recettes SQL concernées : la fixture
  conserve la responsabilité de fermeture. Aucun changement produit.
- Build r47 réussi (9,42 s). Snapshot exact : 639 sources, 180 chemins de revue,
  18 recettes. EXE SHA256
  `f074c4d01a7c9ff47f303dfd881586ec746447c965584b40916aba5fc286fe45`.
  Release source/artefact ROOT r47 SHA256
  `aa272b8107fb222bd8cc9d80be8838ddbbb5a598be7331b76f61ee8d5cb6b89c`.
  Revues indépendantes produit r43/r45 conservées ; corrections de tests
  vérifiées par ROOT. Ne pas présenter cela comme une nouvelle revue indépendante.
- **r47 terminé, 18 PASS** : 15 tests courts (3 purs, 12 SQL), natif privé Ready
  611,529 s, natif famille ACK/rejeu 745,666 s, natif famille Updated/relecture
  785,795 s. Session ROOT `78672` terminée exit0. Ne pas relancer ce parent.
- Archive vérifiée : `ordinary-ready-family-evidence-20261028-r46-r47.tar.gz`,
  1 338 fichiers, 3 822 723 octets, SHA256
  `ffad3ec7ebb04ca48de61bd9bb9a873ab163bb8de0b98adde33f45875380428d`.
  Inventaire correspondant dans `docs/continuation/worktrees-partial-vfs`.

Preuves sous `.layrs-local/validation/provider-family-publication-20261027-r46`
et `r47`. Runner `D:/Layrs/tmp/provider-ordinary-ready-family-run-r47.py`.
Chaque exécution utilise un parent C: neuf. Les fixtures échouées r44/r46
restent conservées et ne sont pas rouvertes. Ces résultats qualifient les seules
portées annoncées ; aucun résultat B Ready/Published/C9 n'en est déduit.

## Suite en préparation, indépendante des artefacts r47

Le commit local `218e625` conserve les trois corrections de tests r47 et cette
note, sans inclure le nouveau lot B Ready/Published. Aucun push.

Le nouveau lot est intégré en source :
partition bornée exacte des reçus, primitives SQL B Ready/Published, nouveaux
lecteurs C/famille sensibles à la phase B réelle, driver physique à deux commits,
nettoyage err-only sans nouvelle validation SQL et invalidation des clones après
rollback échoué. Tests SQL et natifs écrits, pas exécutés. La fermeture positive
de l'owner B reste séparée des résultats de progression ; C reste non admis.

Revue critique T3 GPT-6-Astra xhigh terminée sans finding actionnable ; rapport
`D:/Layrs/tmp/provider-ordinary-source-publication-critical-review-r48.json`.
La cellule ROOT `51` est terminée ; le résultat complet du thread a été lu.
Manifeste de 24 sources :
`D:/Layrs/tmp/provider-ordinary-source-publication-review-inputs-r48.json`, SHA256
`be48500220fdd4f84a8c5c94d17960095afde65734c6928e5a16a2fdc0ccb976`.
Diff SHA256 `7fb83cdbce76fb303667d6ce2c15ab7136692ddbdd3c9a6a253a079f9c06474f`.
Les sources sont restées figées pendant la revue. Recettes r48 proposées : 3 purs,
13 SQL et 2 natifs (nettoyage 240/300, publication B 3600/3660 non mesurés).
Après fin r47, compilation libtest r48 en échec E0603 : import privé depuis le
module partagé. ROOT conserve la preuve fraîche fournie par le driver et la
réauthentifie via require_private, sans exposer l'entrée privée ni réutiliser
une preuve obsolète. Le digest de fixture ci-dessous est également corrigé.
Build r49 réussi (61 s), workspace/all-targets réussi (32,79 s).

ROOT a trouvé et corrigé un défaut dans son nouveau cas natif de nettoyage :
`SET digest='invalid-uncommitted-digest'` viole le CHECK SQL longueur71 avant le
chemin testé. Ce digest est remplacé par `blake3:` +64zéros de longueur71 ; le
rollback et l'assertion d'erreur exacte sont conservés. Les sources originales
revues r48 sont reconstruites depuis le diff épinglé et vérifiées contre leurs
hashes, puis conservées dans le lot r49 avec le diff exact des deux corrections.

Qualification r49 terminée, session ROOT `63777` exit1 : 16 recettes courtes
PASS (3 pures,13 SQL dont publication B en43,002s), puis timeout natif nettoyage
child240 à240,052s, kill/reap confirmés ; publication B non atteinte.
Ne pas relancer ce parent ni rouvrir ses fixtures. Le lot produit est sauvegardé
dans `3717c59` (`wip: preserve ordinary source publication candidate`).
EXE SHA256 `dc237db7d62e0d90b30ccbca76e7721d0ad841f7188cc6dfabc36fa04c4e6dcd`.
Release ROOT r49 SHA256 `743bfdca56d45d8e08060f69e9e02a16a8325e456b20b004fddcef89fdedf14e`.
644 sources,186 chemins sélectionnés,18 recettes. Les lacunes close/rollback OS,
bornes maximales et owner-loss signalées par la revue restent explicites.

r50 conserve le produit r49 identique et ajoute seulement des checkpoints de
progression au test nettoyage, avec child1800/outer1860 diagnostiques. Build
libtest réussi. EXE SHA256
`c2aa43f9f65e0141180680cdba05db033ed41c9bbfc278e01bea7dabebbd89d8`.
Release r50 SHA256
`3d98ca6149bd8cdb43a3f1ea479df33a15d93b35e6f926ea9a7ba7ff1e6b88f6`.
Exécution r50 échoue immédiatement (0,677s) sur accès refusé au parent
`C:/Users/ALEXA`, avant création de fixture ; résultat conservé.

r51 réutilise exactement le binaire et les644sources r50 avec un parent C: neuf
et l'exécution autorisée hors sandbox. Session ROOT **97632**, runner
`D:/Layrs/tmp/provider-ordinary-source-publication-run-r51.py`.
Session97632 terminée exit0 : nettoyage **PASS356,977s**, publication B
Ready/Published et replays **PASS1438,083s**. Ne pas relancer ce parent.
Les16tests courts r49 ne sont pas présentés comme des exécutions r51.

## Lot C10 : historique de préparation et qualification r56/r57

Décision : admission Ordinary8→10 avec témoin complet local des triples, sans
changer le schéma9 Provider. Voir la note `ordinary-autonomy-evidence-gap`.
Travail en cours : permit9/10 et témoin borné, budget explicite, reader C8 autonome
et historique10, writer SQL8→10 et handoff BPublished, family terminal fullwalk,
propagation de la vraie version source dans C10→D, gates de restore/cancellation.
Ces sources évoluent séparément du binaire r51 retenu. L'adaptateur physique et
deux recettes natives sont écrits ; ROOT a enregistré le nouveau module de test.
Compilation libtest r53 en échec : visibilité du validateur d'union, type
ImportRowset/TransitionRowset et branche OrdinaryFrozen manquante. Corrections
ROOT ; r54 PASS. Les régressions ajoutées ont révélé un appel de test à une
méthode privée en r55 ; remplacé par le pattern Ordinary explicite. **r56 libtest
et `cargo check --workspace --all-targets` PASS**. Aucun test/runtime C10 exécuté.

Revue critique SOURCE ONLY r52 terminée (T3 GPT-6-Astra xhigh), cinq findings.
Task id `node:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A35761828-6071-4260-8e0c-b99237f909a8`.
47 sources conservées, manifeste
`D:/Layrs/tmp/provider-ordinary-admission10-review-r52-inputs.json`, SHA256
`081d1dd8b2338c8f2db0f00c2bc7b26339920c3a87b34c8f0202773b1c5c01b5` ; diff SHA256
`4da0d5cfab66dbe62a7fa1edb66b201fb97692a02445666f961e6bf7bb93c35c`.
Snapshot exact dans `D:/Layrs/tmp/provider-ordinary-admission10-review-r52-source`.
Rapport reçu `provider-ordinary-admission10-critical-review-r52.json`.
ROOT a corrigé : visibilité, tri des triples indépendant de l'ordre ManifestId
de la racine, ancêtre10, dispatch historique conversion9|10 et deux gardes
ordinary/reclamation9|10. Régressions dans ordinary_tests et import_validation/tests :
ordres opposés FileId/ManifestId, lectures capture/publication/origine restore,
refus Generation sans scope, C10 interprété comme ancêtre avec reçu D déclaré
et refus d'une closure altérée. Ce dernier cas ne qualifie pas une admission D
réelle ni une chaîne physique D→E.

Contre-revue SOURCE ONLY r56 terminée : cinq corrections confirmées en source,
aucun nouveau défaut concret établi ; intégrité des59sources conforme début/fin.
Cellule ROOT156 terminée ; thread lu jusqu'au résultat final position88.
Manifeste `D:/Layrs/tmp/provider-ordinary-admission10-review-r56-inputs.json`, SHA256
`b01f5c4cf6334cd6e8b3e6e118ede0331c62837c24d361053034984562987f44` ; diff SHA256
`c92c9aad736b271d809cb2fc50fa25f681f463ad905f3fc8aeae7e027590fe77`.
Rapport reçu et lu, acceptation ROOT limitée à l'exécution des recettes :
`D:/Layrs/tmp/provider-ordinary-admission10-critical-review-r56.json`.

Worker `/root/ordinary_admission_schema` a terminé séparément le driver physique
`managed_export/admission.rs`, registration dans `managed_export.rs`, trois
helpers identity-only dans protection/checkout/managed_checkout_fs et les recettes
natives dédiées (helper dans source_publication_tests, nouveau admission_tests).
Ces fichiers, hors revue r52, ont été intégrés puis examinés dans r56.
Pas de réacquisition owner C ; holder emprunte allocation/protection/checkout.
Nouveau B RO indépendant et famille RO traversent C COMMIT/fermeture ; les lectures
fraîches suivent seulement toutes les fermetures positives. Resolve même holder
sansB/writer ; nouvelle ouverture après perte d'owner reste à faire.

Artefacts r56 retenus dans
`.layrs-local/validation/provider-ordinary-admission10-20261028-r56` :650sources,
EXE SHA256 `89b263834d7bf4ab9e76c7e55cce4fda2d2e2c3a6c08aa39646e9257826c3985`,
CodeView age127, EXE/PDB et9sources startup liés. Release ROOT r56 SHA256
`119fc59deb8eb5d66252f37caaaa3f688cd8f2a9a14afea7009370f2bb0294ca`.
Runner `D:/Layrs/tmp/provider-ordinary-admission10-run-r56.py`, session ROOT57640
terminée exit1 :15premières recettes PASS, puis assertion de test incorrecte
comparant l'union complète dans deux ordres différents (17,806s). Natifs non atteints.
Correction ROOT test-only : trier le vecteur attendu par ManifestId pour comparer
l'ordre réel de la racine ; aucune ligne perdue, aucune modification produit.
Build libtest r57 PASS ;650sources comparées au lot r56, seul ce test diffère.
EXE r57 SHA256 `2500ee6d6ac2bef1447535750f400d75e414434672823e6c1d3bfb30a2c1057e`,
release ROOT SHA256 `3e62ff590f1ebd0b5e5e363a1ba1d67443c5e06c53c4177f46b41f72fb5fcc98`.
Runner `D:/Layrs/tmp/provider-ordinary-admission10-run-r57.py`, session ROOT23741
en cours : test complet SQL C10 **PASS42,332s**, premier natif admission
**PASS1580,483s**, second natif ACK/resolve commencé. Ne pas conclure à un PASS
du second scénario avant sa fin. Les15PASS r56 ne sont pas rapportés comme exécutions r57.
Les scripts de préparation/release ont déjà tourné ; ne pas les relancer ni
rouvrir les fixtures. Ne pas compiler ou lancer d'autres tests pendant les natifs.

Suite fonctionnelle après qualification C10 : réutiliser le scénario existant
`session/managed_admission_tests.rs::managed_admitted_ordinary_edit_capture_restore_without_source`
sur une nouvelle destination C10. Le helper `with_admission_hooks` conserve
actuellement tous les pins jusqu'à sa sortie ; faire retourner seulement les
chemins/digests DATA après teardown avant toute nouvelle ouverture mutable.
Rendre indisponibles les chemins sources de cette fixture seulement, après
contrôle des chemins absolus et sans suppression ; vérifier capture, lecture
héritée, restauration/inverse et intégrité de l'admission B. Ne pas prétendre que
le resolve avec holder retenu r56 couvre cette réouverture, ni perte d'owner/crash.

La recette r51 nettoyage localise le coût principal : publication destination de
10,163s à297,008s, admission terminée305,217s, recette terminée356,885s. Le natif
B publication est maintenant terminal PASS. Archive r49–r51 créée et vérifiée :
`ordinary-source-publication-evidence-20261028-r49-r51.tar.gz`, 2014fichiers,
5895640octets, SHA256
`c3e18a26146fd1e1026225970e3ed6dad9fe5768e8eda57c5b4c52f93003ef66`.
Le script `package-ordinary-source-publication-20261028-r49-r51.py` a déjà été
exécuté, ne pas le relancer. Il conserve le timeout r49 et le refus d'accès r50.

Les entrées existantes Staging restent strictes ; les nouvelles entrées
réauthentifient la phase réelle B. Les données Staging reconstruites servent
uniquement aux calculs et ne prouvent pas une phase stockée.

Le programme reste incomplet : admission C10/autonomie,
reprise après perte d'owner/crash, couverture native complémentaire, transitions,
companions/expansion, CLI/Desktop et acceptation complète. Voir la passation
nocturne et `worktrees-partial-vfs-ordinary-publication-next.md` pour le contrat.

## Suite r58 en source pendant les natifs r57

Commits locaux : `31d08d2` conserve le lot C10 revu et l'archive B publication ;
`27fda1f` conserve la correction test-only de l'ordre des manifestes et le PASS SQL.
Aucun push. Les scripts/builds r56/r57 ne doivent pas être relancés.

Trois missions natives terminées, aucun build/test/SQL par les agents :
`ordinary_c10_autonomy_recipe` (implémentation recette),
`ordinary_ready_reopen_scope` (investigation),
`ordinary_terminal_reopen` (implémentation production). ROOT a enregistré le
module de test et intégré la résolution terminale au scénario d'autonomie.

Nouveaux types `managed_ready_reopen::OrdinaryReadyInspection` (C8, owner Shared)
et `OrdinaryTerminalResolution` (C10 inchangé, owner Exclusive), chacun `open`
et `require`. C10 expose `admission_digest` et `admitted`. Aucun B ni writer ;
les holders doivent être détruits avant l'ouverture ordinaire mutable.
Le détachement consommant dans `managed_admission/read.rs` déplace l'ExpectedCheckout
et le CheckoutJournal réellement authentifiés ; vérifie inventaire terminal exact,
frontières, backend/FK, témoin complet et enveloppe inchangée. Les manifestations
canoniques viennent du catalogue familial complet. Les scopes SQL ferment
positivement avant les preuves physiques, puis une nouvelle lecture compare.
La nouvelle admission C8 fraîche avec writer n'est pas implémentée dans ce lot.

Recette proposée, non compilée/exécutée :
`session::managed_ordinary_autonomy_tests::managed_ordinary_c10_reopens_and_captures_restores_after_all_holders_and_ancestors_release`
(child3600/outer3660). Retour de DATA seulement après teardown complet ; chemins
A/B déplacés uniquement après contrôle canonique et exclusion C/famille. Résolution
C10 fraîche sans A/B, puis libération du holder et ouverture publique C, captures
héritées/locales, restauration/inverse, B privé inchangé avec véritable handle et
verrous frais. Le B privé doit rester accessible ; aucun saut silencieux du contrôle.
Après mutations, bytes checkout d'origine rétablis avant le refus terminal attendu,
puis ouverture ordinaire autorisée. Aucun crash, xClose réel défaillant ou D→E.

Contre-revue SOURCE ONLY r58 terminée, cellule ROOT198 clôturée. Thread lu
jusqu'au résultat final position133. Un finding medium R58-F1 : la capture
initiale Ready de B est vide, le test lui demandait à tort des bytes README.
Aucun défaut production concret établi. Huit sources conservées dans
`D:/Layrs/tmp/provider-ordinary-terminal-review-r58-inputs.json`, SHA256
`956681a52b5fd7b7b8f24f64202fa58cedd74b3be668779c20fe2e2d4fa57b0c`, diff SHA256
`c8f29e0581a0fa3e0f5d518614de1937df644c62196ea365ac8e6294d702c18e`.
HEAD de revue `27fda1f480e5986972bf628398fd51dc35c037ed`. Rapport reçu et lu :
`D:/Layrs/tmp/provider-ordinary-terminal-critical-review-r58.json`.
Pas de compilation avant fin des natifs ROOT23741. Les sources r58 restent
séparées du binaire retenu r57. Le script d'archive
`D:/Layrs/tmp/package-ordinary-admission10-20261028-r56-r57.py` est préparé mais
NON exécuté ; attendre la fin du runner avant archivage et vérification.

ROOT a corrigé R58-F1 dans le test uniquement : reconnaissance de la séquence
Ready B réelle, identité B/importée et changes vide ; metadata/origines comparées
pour toutes les captures, streaming seulement des replacements réels, présence
explicite des bytes HISTORY et CURRENT. Aucune capture BASE inventée.
Le test bloque aussi la base privée B avec un handle Windows read/share_mode(0)
pendant toutes les opérations C ; une tentative de lecture doit échouer32.
La comparaison indépendante du baseline B a lieu avant ce blocage et après
libération finale du handle. Checkouts A/B toujours indisponibles.

Contre-vérification r59 en cours, cellule ROOT208 ; portée étroite sur ce seul
test et le guard. Huit fichiers conservés, sept inchangés par rapport à r58.
Manifeste `D:/Layrs/tmp/provider-ordinary-terminal-review-r59-inputs.json`, SHA256
`516a36965bf189c017b92b828c7fe20d43295da20ea42ccf04f0d97c9da73f93`, diff SHA256
`4b12cbb803f328fbe0d8bb1c6b65e25bf9d868e3e08017560e261fa7b8d45262`.
Rapport attendu `D:/Layrs/tmp/provider-ordinary-terminal-correction-review-r59.json`.
Ne pas modifier les huit fichiers pendant cette revue.

Séparément, ROOT a étendu le cas SQL dans `destination_ready/ordinary_tests.rs`
(hors huit fichiers r59) : détachement terminal C10 positif sous le vrai catalogue
familial, append d'une capture locale canonique dans un SAVEPOINT, lecteur générique
acceptant la provenance puis refus par l'inventaire terminal, aucune écriture
de réparation et rollback exact. Non compilé/exécuté ; à qualifier avant le natif
d'autonomie coûteux. Les nouvelles sources restent indépendantes du binaire r57.

## État courant : admission qualifiée, autonomie native r60 en cours

Session ROOT23741 terminée **exit0**. Les trois recettes r57 passent : SQL
42,332s, admission native 1580,483s, ACK retenu/résolution avec holder conservé
1515,529s. Cela ne prouve pas une réouverture après teardown ni un crash.
Archive r56/r57 créée et relue avec vérification de tous les hashes :
`ordinary-admission10-evidence-20261028-r56-r57.tar.gz`, 1464 fichiers,
4460786 octets, SHA256
`d6ece1d7879f753a2442a323073d16694d550a827837917f5be4ea4dacf6b66f`.
L'inventaire correspondant conserve le premier échec d'assertion r56.
Le script d'archive a maintenant été exécuté ; ne pas le relancer.

Contre-revue r59 terminée, thread final lu jusqu'à position34/hasMore=false :
`thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A7fb2e0dd-9df4-47fd-a813-de6b205695da`.
R58-F1 corrigé en source, aucun nouveau finding. Les huit fichiers concordent.
ROOT accepte cette revue de source, sans lui attribuer de résultat runtime.

Build libtest r60 **PASS36,91s** ; `cargo check --workspace --all-targets` PASS.
Artefacts et 652 sources conservés dans
`.layrs-local/validation/provider-ordinary-terminal-20261028-r60`.
EXE SHA256 `c08e2b9d849c8d5044677ebf674b40ec86c7043715e0138c446824c491727e45`,
CodeView age129, neuf checksums de startup vérifiés. Exactement neuf fichiers
Rust diffèrent de r57 : huit revus r59 et le test SQL ajouté par ROOT.
Release ROOT `ordinary-terminal-source-release.json`, SHA256
`04c56c515ceb17a8540eadaf2575cad3a7c67329f99f8a99c5b35ec8ec1f09ce`.
Ne pas relancer `release-ordinary-terminal-r60.py` : ses créations sont exclusives.

Runner `D:/Layrs/tmp/provider-ordinary-terminal-run-r60.py r60`,
**session ROOT48378 en cours**, parent C neuf, séquentiel stop premier échec :
1. SQL C10 avec détachement terminal et refus sans réparation d'un suffixe
   canonique : **PASS38,249s**.
2. Natifs C10 fraîchement rouvert, capture/restauration sous indisponibilité A/B
   et refus de lecture de la base B : **EN COURS**, child3600/outer3660.

Aucun nouveau Cargo, SQL ou autre natif avant fin de cette session. Ne pas
rouvrir la fixture. Les durées sont observées, jamais des garanties maximales.
Le visuel demandé est affiché dans le chat, source conservée hors dépôt dans
`D:/Layrs/outputs/avancement-worktrees-20261028.html`. Il n'arrête pas le goal.
L'investigateur `ordinary_ready_reopen_scope` poursuit en lecture seule la
recherche d'une capacité source B fraîche pour le futur writer d'admission C8.

## Admission fraîche r61 : source figée en revue critique

Le commit local `2ccf824` conserve le candidat terminal r60, ses tests et
l'archive d'admission r56/r57. Aucun push, stash original conservé.
Le script `D:/Layrs/tmp/package-ordinary-terminal-20261028-r60.py` est préparé,
NON exécuté ; il refuse l'archivage tant que le runner n'est pas terminal.

Investigation B fraîche terminée : pas de holder existant complet. Réutiliser
l'enveloppe FS ManagedReadyPins, contrôler explicitement l'identité du control
contre FORMAT, acquérir lifecycle puis owner B Shared, et AdmittedPreflight RO
positivement fermé. Aucun into_handle, Lease::admit, format::Access ou settlement.

Deux agents natifs terminés, source seulement. Nouveau lot de dix fichiers :
`OrdinaryAdmissionSource::open`, `OrdinaryFreshAdmission::open(&source,C,cancel)`
impose B avant l'acquisition Exclusive C ; `commit` et `resolve` renvoient un
digest ou failure step/cause/held. B réel Published et famille sont retenus à
travers COMMIT et close C ; resolve ne consulte ni SQL ni identité B. Le driver
retained, Provider et la résolution terminale C10 restent indépendants.
ROOT ajoute le getter FORMAT étroit, l'enregistrement des tests et un observer
cfg(test), local au thread, RAII, avant read_inner/snapshot et avant identité B.
Aucune modification d'auto-extension, de VFS ou de configuration SQLite globale.

Deux recettes proposées dans `managed_ordinary_fresh_admission_tests.rs` :
- `managed_ordinary_fresh_admission_accepts_advanced_published_b_after_all_publication_holders_release` ;
- `managed_ordinary_fresh_admission_withheld_c_ack_resolves_without_b_reads_or_identity_checks`.
Chacune ignored child3600/outer3660, non mesurée. La seconde prouve une absence
instrumentée de frontière B, pas son absence physique sous pins retenus. Aucun
crash ou échec OS réel n'est revendiqué. Rustfmt isolé PASS ; **non compilé**.

Revue GPT-6-Astra xhigh en cours, cellule ROOT251 (delegate_task mode wait600000).
Manifeste `D:/Layrs/tmp/provider-ordinary-fresh-admission-review-r61-inputs.json`,
SHA256 `4a0d4eee00aa355fbba1c5b0eb5b3be03bc55d3d2a90599e12807a7f5761e21f` ;
diff SHA256 `8f8de85779f42f5bc4c0ca230c8359225b117011986cd1d4a779086e48169756`.
HEAD figé `2ccf824561a511a304332839071576007b143cb5`, dix sources copiées dans
le snapshot adjacent `-source`. Ne pas modifier ces sources ni HEAD pendant revue.
Rapport attendu `D:/Layrs/tmp/provider-ordinary-fresh-admission-critical-review-r61.json`.
Ne pas relancer le script de préparation ; suivre le next de la délégation et
ne pas la poller. Session native r60 ROOT48378 toujours distincte et en cours.

### Résultat terminal r60 et correctif de recette hors périmètre r61

ROOT48378 terminé **exit1** : SQL PASS38,249s, natif FAIL101 après989,252s.
Échec dans l'assertion `managed_ordinary_autonomy_tests.rs:438` : le refus
terminal attendu existe, mais le catalogue familial refuse déjà les racines C
supplémentaires créées par capture/restauration, avant le contrôle d'inventaire
privé. Erreur exacte : `exact protection owner has additional roots`.
Les assertions précédentes du natif ont donc été traversées ; sa réouverture
publique finale et les derniers contrôles B ne sont PAS qualifiés.

Archive r60 créée/vérifiée : `ordinary-terminal-evidence-20261028-r60.tar.gz`,
694 fichiers,2074692 octets, SHA256
`3c8243ccab13820f6814ed6afa85f4a392815a2f13c017fb36f85309ce664908`.
Ne pas relancer le script d'archive ni rouvrir la fixture échouée.
ROOT corrige uniquement le test d'autonomie : assert_eq sur cette erreur exacte,
en conservant bytes d'origine avant refus et toutes les assertions suivantes.
La recette SQL qualifie séparément le refus d'inventaire privé. Ce fichier est
hors des dix sources figées r61 ; aucun fichier de revue ni HEAD ne change.
Build libtest r62 lancé après terminaison du runner, log
`D:/Layrs/tmp/provider-ordinary-fresh-admission-build-r62.log`.

Build r62 terminé **exit101**, un seul E0308 dans
`managed_ready_reopen/ordinary_source.rs:144` : `require_ready_source` renvoie
directement Result<(),ProjectionStoreError> au lieu de convertir en Cause.
Correction mécanique prévue après la revue figée : `require_references(...)?;`
puis `Ok(())`. Ne pas modifier ce fichier pendant r61. Aucun autre diagnostic
compilateur d'erreur ; ne pas transformer ce build en PASS. Prochain build r63.

## Correction R61-F1 et validation suivante

Revue r61 reçue et thread lu jusqu'à position75/hasMore=false, rapport
`D:/Layrs/tmp/provider-ordinary-fresh-admission-critical-review-r61.json`.
Un finding medium confirmé : après ACK C refusé sans échec physique de close,
les lecteurs RO famille/B ordinaires effectuaient encore leurs snapshots FS
de conclusion. Ces snapshots pouvaient remplacer la cause ACK, et le premier
observer ne voyait pas ces opérations. Aucun autre défaut concret établi.

Correction ROOT : nouveau `read_cancellable_strict`, utilisé par les quatre
scopes du nouveau driver frais seulement. La fermeture réelle reste obligatoire ;
toute erreur callback/close est ensuite propagée avant le snapshot final.
Les succès conservent les mêmes comparaisons ; les lecteurs existants gardent
leur comportement de classification/recovery. La cause d'origine reste dans
Cleanup.primary si la fermeture réelle échoue aussi.
Le hook cfg(test) observe maintenant `DatabaseFile::snapshot` ET `verify` avant
FS, en plus de l'entrée RO et de l'identité source. Les contrôles positifs et
le refus après ACK sont étendus. Deux nouvelles recettes SQL courtes dans
`database_io/tests/strict_read.rs` : erreur callback imbriquée ; erreur callback
avec vraie fermeture SQLite BUSY produite par statement retenu. Cette dernière
met le handle en quarantaine jusqu'à sortie du child, sans retry ni simulation
de panne OS. Aucune configuration globale SQLite/VFS ajoutée.

La conversion E0308 r62 est corrigée mécaniquement via `?; Ok(())`.
Build r63 FAIL101 sur comparaison d'un Box<ProjectionStoreError> à une chaîne
dans le nouveau test uniquement ; correction via `.to_string()`.
Build r64 lancé, log `D:/Layrs/tmp/provider-ordinary-fresh-admission-build-r64.log`.
Ces corrections nécessitent une nouvelle contre-revue, pas une acceptation r61.

Build r64 **PASS33,65s**, workspace/all-targets **PASS14,07s**. Aucun test exécuté.
Contre-revue ciblée r65 lancée, cellule ROOT275, GPT-6-Astra xhigh ; attendre son
résultat et suivre son next, sans poller la tâche. Manifeste13 fichiers :
`D:/Layrs/tmp/provider-ordinary-fresh-admission-review-r65-inputs.json`, SHA256
`0af2628d4b38d6df1cba08f4a55df3718143eb5977523b2c658ec158febe8cf5` ;
diff SHA256 `8e33a2b6f245dfc730a9439b22671f9822c50789cb617f3adf1c256eef7b7152`.
Neuf fichiers changent depuis r61/HEAD pour les extensions de périmètre. Tous
les13 fichiers et HEAD2ccf824 restent figés. Rapport attendu :
`D:/Layrs/tmp/provider-ordinary-fresh-admission-correction-review-r65.json`.

Artefacts r66 préparés, **liaison seulement, aucune release ni exécution** :
`.layrs-local/validation/provider-ordinary-fresh-admission-20261028-r66`,657sources,
EXE SHA256 `9e1d64645593e64e61695034158a8df36de7177b9368643ccacd3892f5afd34d`,
CodeView age130, neuf startup checksums et EXE/PDB vérifiés. Script
`prepare-ordinary-fresh-admission-artifacts-r66.py` déjà exécuté, ne pas relancer.
`release-ordinary-fresh-admission-r66.py` préparé, syntaxe parsée, NON exécuté.
Après lecture/acceptation du rapport r65 uniquement, ROOT pourra l'appeler avec
`--root-accept-source-review`. Il refuse les sources modifiées depuis le manifeste.
Programme proposé : les2 régressions SQL courtes, autonomie C10 corrigée, admission
fraîche succès, admission fraîche ACK/résolution. Parent C neuf, séquentiel stop
premier échec. Le runner n'est créé que lors de la release. Aucune session native
ni compilation encore active ; tous les échecs/builds précédents sont conservés.

## État courant r66 : source acceptée, qualification en cours

Contre-revue r65 reçue/lue intégralement jusqu'à position55/hasMore=false :
`thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A11af785e-d59c-4985-9ee4-620ba55e1b30`.
R61-F1 corrigé en source, zéro nouveau finding ;13 fichiers, HEAD/branche et
hashes début/fin concordent. ROOT accepte cette portée pour les cinq recettes
définies ; le rapport ne prétend aucune exécution ou acceptation produit.
Script de release r66 maintenant exécuté (ne pas relancer), SHA256 release :
`7f5ad4198d3743569591f99e4f19645ae727843b305ada99219d10927a1d6e6d`.

Runner `D:/Layrs/tmp/provider-ordinary-fresh-admission-run-r66.py r66`,
**session ROOT7553 en cours**, parent C neuf, arrêt au premier échec :
1. `strict_nested_read_preserves_callback_failure_without_concluding_filesystem_probes`
   **PASS0,534s** ;
2. `strict_nested_read_preserves_primary_and_actual_busy_close_without_concluding_probes`
   **PASS0,118s** ;
3. autonomie C10 corrigée : **EN COURS** ;
4. admission fraîche succès et B avancé : non atteinte ;
5. admission fraîche ACK/résolution sans probes B : non atteinte.

Pas de nouvelle compilation/SQL/natif concurrent avant terminaison du runner.
Le test BUSY couvre une vraie erreur sqlite3_close provoquée par statement
retenu, pas une panne OS ni un crash. L'investigateur ordinary_ready_reopen_scope
étudie en lecture seule la future réouverture de B SourceReady et publication
avec C8/famille Ready, sans réadopter l'allocation ni fabriquer l'image source.
Cette investigation n'est pas une implémentation ni une qualification de reprise.

## Suite source pendant r66 : reprise physique B Ready → Published

Commit local `a2a5549` conserve le candidat admission fraîche, la correction
strict-read et les preuves archivées r60. Aucun push. Goal toujours actif.

L'investigation est terminée : `ordinary_transition::apply_published` vérifie
déjà le vrai B Ready, son frontier et le reçu C exact sans reconstruire l'image
B. Le driver retained est trop couplé à l'ancienne allocation pour servir à une
réouverture fraîche. Le prochain adaptateur possède donc un nouvel owner B
Exclusive, puis C Shared, avec vrais préflights et vérifications physiques.
La capacité doit être recalculée depuis le lecteur autonome C complet
(floor + Jmax, remaining net des octets checkout), jamais fournie par l'appelant.
Les scopes C/famille utilisent le lecteur strict ; le close B positif précède
tout readback. Une résolution explicite relit B Published sans writer ; elle
refuse B Ready ou indisponible. Staging est explicitement hors de ce petit lot.

Deux agents GPT-6.1-Sol réalisent des sources disjointes :
`ordinary_terminal_reopen` possède l'adaptateur/les capacités ;
`ordinary_c10_autonomy_recipe` possède le helper et les recettes natives.
Le helper devra s'arrêter réellement après Ready, fermer B, laisser tous les
holders sortir, puis retourner seulement des DATA. Le hook existant
`before_published` ne suffit pas : il poursuit aujourd'hui vers Published.
ROOT intègre, relit et qualifie. Aucune compilation/SQL/natif supplémentaire
pendant ROOT7553 ; son EXE et ses 657 sources retenues restent immuables.
Ces nouveaux changements ne sont pas encore compilés, revus ou qualifiés.

Progression r66 : l'autonomie C10 corrigée **PASS821,111s**. Les assertions
finales de réouverture publique et d'intégrité B sont cette fois atteintes.
Le scénario admission fraîche avec B avancé est maintenant en cours ; le
scénario ACK/résolution reste à exécuter. Session ROOT7553 toujours exclusive.
Script d'archive `D:/Layrs/tmp/package-ordinary-fresh-admission-20261028-r66.py`
préparé et parsé uniquement ; ne l'exécuter qu'après terminaison du runner.

## Candidat reprise Ready r67 : revue source en cours

Les deux agents d'implémentation ont terminé. ROOT a enregistré le module natif,
relu le diff et renforcé les contrôles positifs d'observation des snapshots et
identités des trois bases. Rustfmt ciblé et diff-check passent. Aucun build ni
test du nouveau lot pendant r66. Les huit fichiers Rust changés et huit fichiers
de contexte sont figés dans
`D:/Layrs/tmp/provider-ordinary-fresh-publication-review-r67-source`.
Manifeste `.../provider-ordinary-fresh-publication-review-r67-inputs.json`, SHA256
`3a658f04430e640b8bcd7db97f5a00beeeef2ded66ff20e90f5576af50fc51c5` ; diff SHA256
`b58e8e0b2e8e70b0ef8e153876a393a073384383252bf09a382de55665d7c38c`.
HEAD attendu `a2a55497ee2dad70ca8098e59f5faf257ade2321`.
Revue critique GPT-6-Astra xhigh lancée, cellule ROOT312 ; rapport attendu
`D:/Layrs/tmp/provider-ordinary-fresh-publication-critical-review-r67.json`.
Suivre le résultat de l'appel en attente, sans polling de la tâche déléguée.

L'investigation du lot suivant Staging→Ready est également terminée et conservée
dans [la proposition dédiée](worktrees-partial-vfs-ordinary-fresh-staging.md).
Elle décrit une image B fraîche réelle et la réutilisation des kernels existants,
sans faux Access ni allocation adoptée. Ce lot n'est pas implémenté ; ne pas le
confondre avec le Ready→Published actuellement en revue.

Progression runtime r66 : admission fraîche après libération de tous les
holders et suffixe cohérent B **PASS785,869s**. Le dernier scénario
`managed_ordinary_fresh_admission_withheld_c_ack_resolves_without_b_reads_or_identity_checks`
vient de démarrer dans ROOT7553. Les quatre premières recettes r66 passent ;
ni le lot complet ni l'archive r66 ne sont encore terminés.

Revue critique r67 terminée : aucun défaut concret, 16 inputs et 10 dépendances
supplémentaires concordants au début/à la fin. Thread lu jusqu'à position55,
hasMore=false :
`thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3Ad89f62d8-f972-4b89-8ff2-3c3bc204938f`.
Rapport JSON ci-dessus lu intégralement. ROOT accepte cette revue de source,
sans résultat de compilation ni runtime du candidat. Cellule312 terminée.

Après terminaison de ROOT7553 seulement : build libtest et check workspace
prévus sous logs `provider-ordinary-fresh-publication-build-r68.log` et
`provider-ordinary-fresh-publication-check-r68.log` dans `D:/Layrs/tmp`.
Scripts `prepare-ordinary-fresh-publication-artifacts-r69.py` et
`release-ordinary-fresh-publication-r69.py` créés et parsés, NON exécutés.
Ils vérifient les sources r67, les 659 inputs attendus, les deux logs de build,
les cinq succès r66 et exactement huit changements avant de lier puis libérer
trois recettes SQL existantes et les deux natives nouvelles. La release exige
l'argument explicite ROOT `--root-accept-source-review`.

Visuel actualisé montré inline : `D:/Layrs/outputs/avancement-worktrees-r66.html`,
HTML preview inspecté puis html_render, sans serveur. C'est un instantané
(avant retour de la revue r67), pas un suivi automatique.

## r66 terminé : cinq recettes PASS, preuves archivées

ROOT7553 est terminé **exit0**. Dernier scénario ACK/résolution **PASS746,214s**.
Ordre complet : SQL strict0,534s ; SQL close BUSY0,118s ; autonomie C10
821,111s ; admission fraîche B avancé785,869s ; ACK/résolution sans B746,214s.
Le PASS porte sur ces recettes exactes et leurs limites, pas sur le programme.

Archive créée et relue avec vérification de chaque empreinte :
`docs/continuation/worktrees-partial-vfs/ordinary-fresh-admission-evidence-20261028-r66.tar.gz`,
711 fichiers,2106092 octets, SHA256
`0e13f4e378a8119793930f60059b0d9d6baa76d91c01bbc32503b6a96df30999`.
Inventaire adjacent `ordinary-fresh-admission-inventory-20261028-r66.json`.
Le script d'archive est déjà exécuté, ne pas le relancer. EXE/PDB et fixtures
restent locaux et ne sont pas archivés ; aucun ancien état de fixture rouvert.

Compilation r68 lancée après terminaison native, **session ROOT85188**,
`cargo test -p layrs-store-local --lib --no-run`. Aucun test lancé. Vérifier
son résultat avant workspace/check, liaison r69 et release ROOT.

## État courant r69 : candidat compilé, exécution en cours

Build libtest r68 **PASS32,43s**, workspace/all-targets **PASS12,89s**.
ROOT85188 et ROOT7413 terminés exit0. Aucun changement Rust depuis r67.
Artefacts r69 retenus :659 sources, neuf startup checksums, EXE/PDB concordants,
CodeView age131. EXE SHA256
`3ca51aa2fbdb45aae668135da2702f99ff831c1f792c05240b6335c624f43b99`.
Les scripts de préparation et de release r69 sont maintenant exécutés ; ne pas
les relancer. Les 16 inputs r67 et 10 dépendances supplémentaires concordent ;
les huit seuls changements depuis r66 sont ceux revus. Release ROOT SHA256
`25e314dcf8cf97fa4a7e70d97740a3aee99b267bd8ec86f58e583f3220ecee57`.

Runner `D:/Layrs/tmp/provider-ordinary-fresh-publication-run-r69.py r69`,
**session ROOT2313 en cours**, parent C neuf, séquentiel stop au premier échec :
1. publication SQL Ordinary complète : en cours ;
2. lecteur autonome Provider positif : non atteint ;
3. refus autonome suffixe/journal/phase/débit : non atteint ;
4. publication fraîche B Ready→Published puis replay : non atteinte ;
5. ACK B refusé puis résolution Published nécessitant B : non atteinte.

Pas de Cargo/build/SQL/natif concurrent avant terminaison. Les sources de la
future reprise Staging peuvent avancer séparément du binaire retenu r69, en
respectant la proposition dédiée et une nouvelle revue. Tous les agents natifs
sont terminés ; aucune tâche T3 en attente. Goal actif ; pas de push/PR.

Checkpoint local `e2a1ac460fb422b46b784796ce00f521443f6695` : candidat r67/r68,
archives r66 et documentation conservés, stash original intact. Après lancement,
les trois premières recettes SQL r69 passent :24,177s ;0,873s ;1,226s.
Le natif `managed_ordinary_fresh_publication_advances_released_ready_and_replays_published_without_writer`
est en cours dans ROOT2313 ; le natif ACK B/résolution n'est pas encore atteint.
Ne pas relancer le runner ni les scripts de préparation/release. La passation
présente ce dernier état ; le binaire retenu reste celui accepté avant commit.

## Reprise Staging en implémentation pendant r69

Tour goal précédent = progrès réel (r66 cinq PASS, archive, commit candidat,
r69 trois SQL PASS). Goal revérifié actif. ROOT2313 confirmé encore vivant par
son handle ; pas de redémarrage ni autre exécution native/build simultanée.

Trois agents GPT-6.1-Sol, sources disjointes :
- `ordinary_fresh_image_parts` : `capture/scan.rs` et `managed_export/image.rs`,
  extraction des kernels physiques sans contourner les anciens ContentScope ;
- `ordinary_terminal_reopen` : nouveau `managed_export/staging_reopen.rs`,
  wiring et interfaces minimales source exclusive/C inspection ;
- `ordinary_c10_autonomy_recipe` : helper arrêt réel avant B Ready et nouveau
  `session/managed_ordinary_fresh_staging_tests.rs`.

API convenue : `OrdinaryStagingPromotion::open(B,C,transaction_id,cancel)` puis
`promote_ready(self,close,cancel)` ; erreur garde `step/cause/held`, succès expose
Ready/receipt/digest DATA. Construction strictement Staging. Aucun replay ou
resolve de cet objet qui traiterait Ready comme Staging : après ACK incertain,
libération explicite du holder puis driver distinct `OrdinaryReadyPublication`
sur le vrai état. Les anciennes APIs Shared/Ready/Provider restent inchangées.
Tests proposés : image B réellement modifiée puis restaurée avant acquisition
réussie ; succès Ready exact ; ACK B refusé, arrêt des probes et ouverture fraîche
du driver Ready après teardown. Pas de claim crash/OS close ou arbre complexe.

Script `D:/Layrs/tmp/package-ordinary-fresh-publication-20261028-r69.py` préparé
et parsé, NON exécuté ; attendre la terminaison de ROOT2313, même en échec.

Progression r69 : natif publication fraîche et replay **PASS812,201s**.
Dernier natif ACK B/résolution en cours dans ROOT2313. Les trois SQL passent.


Suite plus ancienne : `worktrees-partial-vfs-resume-archive-2-2026-10-28.md`.
