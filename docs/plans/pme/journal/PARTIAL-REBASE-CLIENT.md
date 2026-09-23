# Fusion partielle — client, CLI et Desktop — 23 septembre 2026

## Implémenté

Le client prépare une fusion de la capture locale avec la tête distante unique.
Chaque ancienne et nouvelle valeur est téléchargée sous autorisation actuelle
du chemin exact. Les absences sont reclassifiées sous la même autorité ; les
chemins protégés restent réservés. Aucun Step incomplet n'est admis localement.

Les textes compatibles sont réconciliés par le moteur Lens natif borné. Les
conflits, binaires et valeurs dépassant le budget de fusion demandent un choix
explicite base/local/distant/suppression. Les octets sont dans des spools privés,
les métadonnées du brouillon dans SQLite. Une nouvelle préparation est exigée
si la capture, le checkout, les droits ou la View changent. L'opération achevée
lie son identifiant au brouillon et à tous les choix pour retrouver le résultat
après réponse perdue. Aucune requête réseau sous le verrou du journal local.

CLI : `remote partial-rebase-preview CAPTURE LAYER --output PREVIEW.json`,
`remote partial-rebase-apply PREVIEW.json CHOICES.json`, `partial rebase-draft`
et `partial resume-rebase`. CHOICES est un objet chemin → base/local/remote/delete,
vide si aucun conflit. Desktop : préparation, recharge du brouillon, choix de
chaque conflit et application, puis publication de la nouvelle capture.

## Vérifié

- Recette HTTPS/PostgreSQL complète : 1 PASS, 0 ignoré, 107,25 s.
  `D:/Layrs/tmp/rebase-https-20260923.log` : fusion disjointe, brouillon après
  réouverture, rejeu exact, conflit non résolu, View devenue périmée, choix puis
  publication, ainsi que les recettes de transfert100Mio et ACL existantes.
- Trois tests client : textes compatibles/incompatibles, absence/fichier vide,
  modification/suppression, binaires et dépassement du budget.
- 73 tests Store3 par l'implémenteur ; Clippy client/CLI/store sans avertissement.
- TypeScript Desktop : PASS. Recette réelle Desktop et tests Linux en cours.

## Limites restantes

La fusion travaille sur des fichiers ordinaires et une tête distante choisie.
Les changements de droits nécessitent d'abord la procédure de réception ; les
transformations structurelles et bases multiples ne sont pas implémentées ici.
Le brouillon conserve les variantes, mais les choix non appliqués sont encore
dans le formulaire ou le fichier CLI. Pas de fausse annonce de persistance des
choix du formulaire après fermeture.

La limite de threads d'agents empêche une nouvelle revue indépendante. Les
tests et relectures de la racine ne constituent pas cette revue indépendante.
