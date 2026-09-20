# Reprise locale des téléchargements V3

`NativeRemote` utilise des checkpoints privés hors Worktree pour les objets
Identity dont le serveur autorise les ranges. `with_download_cache(path)` permet
un dossier dédié explicite ; son parent doit exister. Par défaut :
`%LOCALAPPDATA%/.layrs-download-resume-v1` sur Windows et
`$HOME/.layrs-download-resume-v1` sur Unix. Aucun repli vers un dossier moins
protégé : ACL utilisateur exacte sous Windows, propriétaire courant et0700 sous
Unix, liens/reparse points refusés par les primitives filesystem épinglées.

Ce cache contient des **octets de projet sensibles**, protégés par les droits
locaux et non chiffrés. Il ne contient ni bearer, ni token de capability, ni
resumeToken. Une empreinte BLAKE3 de la capability sert seulement à documenter
le segment ; elle ne vaut pas autorisation et ne bloque pas son renouvellement.

La clé lie origine normalisée, Space, principal, epoch d'autorisation, claim et
descripteur exact. Chaque reprise demande un nouveau bootstrap/plan/capability ;
les anciens jetons ne sont jamais rejoués. Changement de principal, d'epoch,
d'origine ou de claim : aucun ancien préfixe consommé. Le serveur doit encore
autoriser le GET, même si le préfixe local couvre déjà tous les octets.

Pendant un objet, seul un refus404 `transfer_unavailable` avec date de capacité
déjà passée selon l'horloge cliente permet une nouvelle requête du même claim.
Le plan renouvelé doit conserver principal, Space, epoch, client, sélections de
manifests et ensemble exact des descripteurs ; le GET suivant utilise son nouveau
token. Deux renouvellements maximum par objet, dans la deadline réseau totale
de120s ; les capacités restantes du même plan utilisent aussi ce renouvellement.
Révocation, epoch changé, framing ou identité corrompue : refus sans ce retry.
Une horloge cliente retardée peut imposer un nouvel appel explicite ; aucun droit
n'est déduit de cette horloge. Le serveur reste seul autorisateur du GET.

La façade serveur conserve900s par défaut. Son builder
`with_plan_lifetime_seconds` permet seulement de raccourcir cette durée entre1
et900s pour de nouveaux plans. Les plans finalisés restent immuables, y compris
leur date d'expiration ; aucune migration ou réécriture de grants n'est requise.

Chaque checkpoint immuable contient au plus1Mio de données, les offsets, le
digest des données et le digest du checkpoint précédent. Publication du fichier
après fsync puis renommage sans remplacement et synchronisation du répertoire.
Au redémarrage, tous les checkpoints sont relus et contrôlés avant reprise ;
trou, octets corrompus ou binding incorrect font échouer cette tentative. Les
checkpoints corrompus reconnus sont nettoyés pour permettre un nouvel essai frais.
Les fichiers étrangers provoquent un refus et sont conservés.

Limites :256Mio logiques de fichiers du cache, abandonnés et temporaires inclus,
128entrées,128checkpoints par objet. Ce quota ne représente pas l'occupation
physique exacte du filesystem. L'éviction supprime seulement une entrée du cache
inactive, sous verrou global, après validation de son marqueur. Les données
incomplètes peuvent donc être évincées ; la reprise redémarre alors l'objet.
Les objets entièrement téléchargés sans interruption ne créent pas de checkpoint
inutile. Un arrêt avant publication du checkpoint peut perdre le segment courant.
Une métadonnée locale incomplète/non reconnue est refusée explicitement.

Le décodage et l'identité canonique complète restent obligatoires après la
réponse HTTP. L'admission locale attend toujours la fermeture vérifiée. Zstd ou
absence de ranges : spool éphémère uniquement. Les bornes64Mio/objet et256Mio de
fermeture restent inchangées ; ce mécanisme n'est pas le streaming global du
Store ni un checkpoint de toute la traversée de graphe.

Qualification : tests Windows et Linux de droits/quota/intégrité ; recette
HTTPS/PostgreSQL `LAYRS_TEST_DURABLE_DOWNLOAD_ONLY=1` dans
`native_clients_v3` avec reprise dans un nouveau processus OS, préfixe réellement
réutilisé, bearer refusé, mutation réelle epoch, corruption et absence de Store
avant fermeture. Le lot complémentaire `LAYRS_TEST_TRANSFER_RESTART_ONLY=1`
qualifie séparément l'expiration chronologique et les arrêts de processus aux
frontières transaction/reçu ; voir le journal T4 pour le résultat exact. Les
arrêts forcés à chaque instruction filesystem et redémarrages serveur restent
des preuves distinctes.
