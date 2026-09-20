# HTTPS1Gio — qualification complète du20septembre2026

Résultat : **PASS**, 1092,828s de recette (18min12,828s), 1105,436s pour le
processus (installation/authentification/cleanup inclus). Exécutée seulement après
la [recette100Mio verte](T4-streaming-100mib-20260920.md) et la validation locale
fsck1Gio Windows/Linux. Aucun plafond de l'ancienne API en mémoire n'a été levé.

```text
node scripts/test-native-streaming-volume.mjs --binary D:/Layrs/layrs/tmp/native-streaming-20260920-204610.exe --report D:/Layrs/layrs/tmp/native-streaming-1gib-20260920-204911.jsonl --sizes 1024 --restore
```

URL PostgreSQL privée fournie par environnement, absente du rapport. SHA256 du
binaire copié : `0f847e0db3b1a7530c596e6671cf5737c809650283b3fdaf9fb9545ff9601c88`.
Rapport JSONL et `.log` conservés sous `tmp/`, sans écraser les mesures100Mio.
Code de sortie0, aucun timeout ni phase skipped. Le harness a nettoyé sa base,
son rôle et ses dossiers UUID après le résultat.

| Phase | Temps |
|---|---:|
| Génération de1024blocs uniques de1Mio | 4,989s |
| Initialisation/capture | 72,475s |
| Provision | 0,212s |
| Publication | 139,440s |
| Clone indépendant | 168,481s |
| Comparaison exacte clone + FileId | 2,975s |
| Receive inchangé, aucune Operation nouvelle | 36,468s |
| Capture après modification du bloc central | 105,514s |
| Publication incrémentale | 103,563s |
| Réception modifiée | 333,181s |
| Comparaison exacte réception + FileId | 2,685s |
| Restauration de la Step initiale | 116,510s |
| Comparaison exacte restauration + FileId | 2,702s |

Fichier1073741824octets, chunks de capture1Mio. Les graines du générateur
dépendent de la taille du cas et de l'index : les blocs ne réutilisent pas ceux du
cas100Mio. Modification exacte d'un bloc1Mio central, longueur totale inchangée.
La fixture génère avec1Mio et compare en flux avec2Mio, sans garder tout le fichier.
Chaque identité est aussi comparée à l'entrée File du Tree canonique de sa Step.

| Version | BLAKE3 brut | FileId |
|---|---|---|
| Initiale/restaurée | `0f451383865c5928d97e1f698fb53514e4222a5ae21f4e92cbd36ead427ac54b` | `blake3:73502d4804fdb12a44c3ae78b7b47fe1c913e5541011b1ec99530d2ad2cde55f` |
| Modifiée | `5c0c521cfb47127bd3add85a39fe75d511388c4191cb5f04ce7b132af771bd78` | `blake3:ab71e195ba543c832227927010823f235127628fc4fabbedebc21922f0472836` |

MaximumRSS échantillonné62156800octets (59,28Mio), privé24219648octets
(23,10Mio), picRSS OS cumulé62513152octets (59,62Mio). Les mesures regroupent le
serveurHTTPS et les clients natifs **dans un même processus**. PostgreSQL, cacheOS
et autres processus sont exclus. Les échantillons50ms peuvent manquer un pic bref ;
le picOS est cumulatif. Windows/debug, une exécution locale avec qualifications
simultanées : aucune garantie de débit production ni comparaison concurrentielle.

Ce résultat prouve publication, clone, contrôle dirty, capture du changement,
réception et restauration en flux pour cette fermeture, avec contenu exact. Il
ne qualifie pas toutes les APIs historiques qui retournent des Vec, un checkout
partiel, la fusion de fichiers1Gio, ou toutes les distributions de manifests.
Les manifests anciens en chunks64Kio ne sont pas le format de cette recette.
