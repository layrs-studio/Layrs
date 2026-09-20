# Recette HTTPS — fichiers uniques de 100 Mio et 1 Gio

État initial : préparée avant disponibilité des parcours en flux. Après leur
livraison et correction du contrôle dirty, la recette100Mio complète avec restore
est [qualifiée dans un rapport séparé](T4-streaming-100mib-20260920.md).
La [recette1Gio complète](T4-streaming-1gib-20260920.md) est également qualifiée,
avec son propre rapport neuf. Le présent document décrit le protocole, sans
extrapoler ces mesures à tous les parcours ou aux performances de production.

Le helper `streaming_volume_drill.rs` utilise un projet indépendant par taille et
le vrai serveur HTTPS/PostgreSQL du test `native_clients_v3`. Il génère chaque
bloc de 1 Mio par BLAKE3 XOF à partir d'un index et d'une taille de cas distincts
(les deux tailles ne réutilisent donc pas leurs blocs) ; aucun motif limité à
200 valeurs ou réutilisation intentionnelle des mêmes chunks. Un ensemble borné
d'empreintes vérifie l'unicité des blocs. Le générateur conserve 1 Mio, la
comparaison exacte 2 Mio ; aucune lecture entière du fichier dans la fixture.

Chaque taille passe par génération, initialisation/capture, publication, clone,
vérification de tous les octets en flux et du FileId dans le Tree canonique,
réception inchangée sans nouvelle Operation, modification d'un seul bloc,
capture/publication, réception et vérification du nouveau contenu. Une empreinte
BLAKE3 brute et l'identité FileId séparée par domaine sont conservées. La
restauration de l'ancienne Step est optionnelle, uniquement lorsque ce parcours
produit utilise lui aussi une matérialisation en flux.

Un rapport JSONL est créé avec `create_new`, puis synchronisé au début et à la fin
de chaque phase. Les paniques laissent un événement failed sans copier leur
payload ou des secrets. Un arrêt brutal laisse la dernière phase running ; le
script ajoute son résultat de processus après la terminaison. Il conserve un log
borné à 16 Mio dans un autre fichier neuf, sans l'afficher à la console.

Windows : échantillons du working set et des octets privés toutes les 50 ms, plus
pic working set depuis le lancement. Linux : VmRSS et VmHWM de `/proc/self/status`.
macOS non instrumenté. Les compteurs concernent **le même processus contenant le
serveur HTTPS et les clients natifs**, excluent PostgreSQL et le cache de l'OS ;
le pic cumulé n'est jamais présenté comme un delta par phase. Les pics brefs
peuvent échapper à l'échantillonnage. Exécution debug locale, sans garantie PME.

Préparer le binaire avec `cargo test --offline -p layrs-server --test native_clients_v3 --no-run`,
puis, après le signal de disponibilité des implémentations et avec la variable
privée `LAYRS_TEST_DATABASE_URL` déjà configurée :

```text
node scripts/test-native-streaming-volume.mjs --binary CHEMIN_TEST_EXE --report NOUVEAU_RAPPORT.jsonl --sizes 100,1024
```

`--restore` active le parcours de restauration, sinon un événement skipped
l'indique explicitement. `--timeout-minutes` borne le processus (90 par défaut,
maximum 180). Le SHA256 du binaire figure dans le résultat du lanceur. Pour une
reprise après échec, choisir un **nouveau rapport** et `--sizes 100` ou `1024` ;
les résultats précédents ne sont ni écrasés ni convertis en succès. Le nettoyage
de la base et des fichiers UUID reste celui du harness natif ; un kill externe
peut demander le nettoyage explicite ultérieur de cette fixture identifiée.
