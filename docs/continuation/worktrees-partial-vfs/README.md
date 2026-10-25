# Reprendre le programme Worktrees / partial VFS

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
