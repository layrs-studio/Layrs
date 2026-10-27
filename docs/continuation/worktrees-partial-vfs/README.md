# Reprendre le programme Worktrees / partial VFS

## Complément ouverture ordinaire et cycle modifiable — r16 à r18

Le jalon r16 qualifie la réouverture publique du schéma 9 et la fermeture
explicite de SQLite sur les chemins de succès et d'erreur testés. Le scénario
natif r18 qualifie préparation hors ligne, modifications, captures, lectures,
comparaison, réouverture et restaurations avec le chemin source indisponible.
Les détails et limites sont dans le [checkpoint](../../plans/worktrees-partial-vfs-checkpoint.md)
et le [contrat](../../plans/worktrees-partial-vfs-managed-admission-contract.md).
La reprise des restaurations interrompues, les historiques inverses importés,
les autres transitions, companions, expansion, nettoyage et parcours produit
restent à terminer. Les sections plus anciennes ci-dessous conservent l'état
historique de chaque jalon.

`ordinary-evidence-20261027-r16-r18.tar.gz` contient 1 744 fichiers vérifiés :
sources figées, revues, recettes et journaux de compilation/exécution, ainsi
que la révision r17 remplacée sans exécution. Inventaire :
`ordinary-inventory-20261027-r16-r18.json`. Archive : 4 768 023 octets ; SHA-256
`e3ec0c58cc3375bdc6bf8b4df9ffc28e715e8d5ba2757c3b0754f1a94b07298e`.
Exécutables/PDB, bases SQLite et fixtures natives restent exclus. Ce paquet
préserve des preuves historiques et ne qualifie pas l'hôte d'un autre poste.

Sur un poste où ces preuves sont absentes :

```powershell
tar -xzf docs/continuation/worktrees-partial-vfs/ordinary-evidence-20261027-r16-r18.tar.gz
```

Les restrictions de restauration ci-dessous s'appliquent.

## Complément admission et réconciliation — r11 à r13

Le lecteur de provenance schema9, le writer transactionnel d'admission, les
budgets et les noyaux DATA d'observation/réconciliation sont qualifiés dans les
périmètres décrits au [checkpoint](../../plans/worktrees-partial-vfs-checkpoint.md).
L'admission physique durable et l'ouverture ordinaire modifiable restent à
raccorder ; le [contrat](../../plans/worktrees-partial-vfs-managed-admission-contract.md)
précise les six raccordements nécessaires.

`admission-evidence-20261027-r11-r13.tar.gz` contient 1 720 fichiers : sources
figées, manifestes, recettes et journaux, y compris les échecs conservés.
Chaque membre a été relu et vérifié contre son empreinte dans
`admission-inventory-20261027-r11-r13.json`.
Archive : 4 757 898 octets ; SHA-256
`8b5476fe031db222eda6a3384e2e53a5c8e911c14e699ca22cd48010be000c9e`.

Sur un poste où ces preuves sont absentes :

```powershell
tar -xzf docs/continuation/worktrees-partial-vfs/admission-evidence-20261027-r11-r13.tar.gz
```

Les restrictions de restauration ci-dessous s'appliquent. Les binaires/PDB,
bases SQLite et fixtures natives restent hors archive. Ces résultats ne valent
pas qualification de l'hôte d'un autre poste.

## Complément du 27 octobre 2026

Les commits locaux `ddf31d3` et `0afbcc0` ajoutent les publications familiale et
source, la capture après Published, les lecteurs Ready autonomes et la
réouverture physique en lecture. Leur état courant est décrit par le
[checkpoint](../../plans/worktrees-partial-vfs-checkpoint.md) et le
[contrat d'admission](../../plans/worktrees-partial-vfs-managed-admission-contract.md).
L'admission ordinaire mutable, les companions, l'expansion et les parcours
produit ne sont pas terminés.

Le complément `managed-evidence-20261027.tar.gz` transporte 12 346 fichiers
de sources figées, manifestes et journaux du 27 octobre, jusqu'à l'artefact r10.
Chaque fichier a été relu depuis l'archive et comparé à son SHA-256.
`managed-inventory-20261027.json` donne les chemins, tailles et empreintes.
Archive : 33 556 498 octets ; SHA-256
`8da87e87a87f379f03b7c7f7140495c22b30a9b48eb465c10bb92be35697c3e5`.

Sur un poste où ces preuves sont absentes :

```powershell
tar -xzf docs/continuation/worktrees-partial-vfs/managed-evidence-20261027.tar.gz
```

Les mêmes restrictions de restauration et d'hôte ci-dessous s'appliquent.
Les échecs r4/r5/r6/r9/r10 restent conservés ; leur présence dans l'archive
n'est pas une acceptation. Les sources non modulées présentes dans certains
inventaires sont des brouillons, sans qualification. L'archive ne contient
ni exécutables/PDB, ni bases SQLite, ni fixtures natives. Elle ne remplace
pas `evidence.tar.gz`, qui conserve le jalon historique writer R7.

## Paquet historique writer R7

Le commit `0eba2e5` contient les sources, tests et scripts du programme. Son
premier push omettait les contrats et dossiers de revue conservés dans le
répertoire ignoré `.layrs-local/validation`. Ce complément transporte ces
documents et leurs copies de sources, sans changer le code du produit.

## Restaurer les documents locaux manquants

Depuis la racine du dépôt, sur la nouvelle machine où ces dossiers sont absents :

```powershell
tar -xzf docs/continuation/worktrees-partial-vfs/evidence.tar.gz
```

L'archive contient des fichiers ordinaires sous `.layrs-local/validation/`, avec
leurs octets d'origine. Elle ne contient aucun lien symbolique ou chemin absolu.
Ne pas extraire par-dessus un dossier de preuves déjà présent sans comparer les
empreintes : les preuves historiques doivent rester immuables.

`inventory.json` donne les chemins relatifs, tailles et SHA-256 des fichiers
transportés. Les documents historiques conservent leurs chemins absolus d'origine
et leurs empreintes. Pour lire un chemin commençant par
`C:/Users/ALEXA/.t3/worktrees/Layrs/feature-worktrees-partial-vfs/`, utiliser le
même suffixe depuis la racine du nouveau checkout. Ne pas réécrire les manifestes
historiques. Les chemins externes au dépôt désignent l'ancien environnement.

## Ordre de lecture

Le supplément `admitted-context-evidence-20261027-r14-r15.tar.gz` contient les
sources figées et résultats des lecteurs admis, du contrôle d'aliasing des
répertoires temporaires et de l'admission physique avec reprise après perte
d'accusé. Il conserve aussi le premier échec de fixture natif r14. Inventaire :
`admitted-context-inventory-20261027-r14-r15.json`, 1 167 fichiers vérifiés.
Archive : 3 214 056 octets ; SHA-256
`c6dbc24e33988cd69c977ee7fd079303385c4c322d7197a1de4599aa4f54181d`.
Les exécutables, PDB, bases et fixtures natives restent exclus. Ce supplément ne
qualifie pas l'ouverture ordinaire modifiable ni le programme complet.

Depuis la racine du dépôt, extraction des preuves historiques :

```powershell
tar -xzf docs/continuation/worktrees-partial-vfs/admitted-context-evidence-20261027-r14-r15.tar.gz
```

1. [Point d'étape](../../plans/worktrees-partial-vfs-checkpoint.md), puis
   [programme](../../plans/worktrees-partial-vfs.md),
   [storage](../../plans/worktrees-partial-vfs-storage.md) et
   [convergence](../../plans/worktrees-partial-vfs-convergence.md).
2. Après extraction :
   `.layrs-local/validation/l5-store-stage1b-consolidated-contract-r4.md`.
   C'est le contrat durable à préserver. Ses formulations de proposition
   décrivent sa date d'écriture ; les décisions ultérieures sont distinctes.
3. Les dossiers `l5-store-stage1b-writer-design-r1-*` et
   `l5-store-stage1b-writer-design-r2-*` : proposition, addendum, remplacement R2,
   plans et décisions de revue.
4. Le dossier `l5-store-stage1b-writer-r7-bdbc821b61a64108b8355ee9771f6df1`,
   notamment `freeze.md`, `review-manifest.json`, puis
   `owned-runtime-dea358a7a7a049b588aa5d23291eeb11/review-disposition-r1.json`.
5. [Proposition de suite exportée](../../plans/worktrees-partial-vfs-next-slice-proposal.md).
   Elle nécessite encore sa revue et son autorisation de mise en œuvre.

## État et limites du transfert

Les 156 sélections natives writer R7 ont une acceptation bornée jusqu'à
`control_published`, membre de famille toujours `initializing`. Ce n'est pas
l'acceptation du programme complet ni l'activation CLI/Desktop. Les échecs
historiques restent des échecs ; les résultats purs R5 gardent leur provenance.
La course de namespace et l'optimisation restent HOLD.

Le paquet est destiné à poursuivre la conception, les revues de source et
l'implémentation. Il exclut les exécutables/PDB, sorties de compilation, caches,
environnements locaux, bases et fixtures natives. Il ne permet donc pas de
revalider toutes les empreintes binaires des anciens manifestes ni de transférer
l'acceptation de l'ancien hôte au nouvel ordinateur. Les sources et résultats
transportés restent des preuves historiques, pas un nouveau test.

Ne pas lancer automatiquement les anciennes commandes de qualification : leurs
chemins, binaires, identités natives et autorisations appartiennent à leurs runs.
Une nouvelle qualification demande un build et une disposition adaptés au nouvel
hôte, des parents frais et la file explicitement approuvée. Aucun nettoyage,
réemploi des fixtures, retry, élargissement de droits ou activation produit ne
découle de ce transfert.
