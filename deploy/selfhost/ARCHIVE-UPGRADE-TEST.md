# Recette de mise à jour des archives CLI

Exécutée sous Windows entre `qualified2` et `qualified3` le 20 septembre 2026 :
lecture de l'historique exact, nouvelle capture, refus d'un binaire altéré et
retour au snapshot sauvegardé au même emplacement réussis. Rapport conservé :
`tmp/archive-upgrade-windows-qualified3/report.json`. La nouvelle capture reste
dans l'état d'incident préservé ; elle ne fait pas partie du retour arrière.
Une nouvelle paire d'archives et l'exécution Linux doivent être qualifiées
séparément ; ce résultat ne vaut pas validation de tout successeur.
Elle qualifie uniquement deux snapshots distincts utilisant déjà le Store natif
format2 et V3 ; elle ne qualifie ni migration PostgreSQL, ni conversion de format,
ni installation MSI, ni environnement de production.

La commande est identique sous Windows x64 et Linux x64 avec Node disponible :

```text
node scripts/test-native-upgrade.mjs --old CHEMIN_QUALIFIED2.tar.gz --old-sha256 SHA256_ANCIENNE_ARCHIVE --new CHEMIN_NOUVELLE_ARCHIVE.tar.gz --new-sha256 SHA256_NOUVELLE_ARCHIVE --output DOSSIER_NEUF
```

Obtenir les digests depuis le canal de livraison de confiance. Le script exige
des archives, des sources et des binaires CLI distincts ; recomprimer le même
binaire ne constitue pas un essai de mise à jour. Les archives d'entrée restent
inchangées. Le dossier de sortie doit être neuf, avec un parent déjà existant.
Ne fournir aucun dossier de travail utilisateur.

Avant toute exécution : SHA256 de chaque archive, tar régulier borné (256Mio
compressés,512Mio décompressés,128entrées, arborescence plate), refus de liens,
traversées, noms spéciaux Windows, doublons, fichiers spéciaux et extensions PAX,
puis manifeste et hashes de chaque fichier. La recette emploie le vérificateur du
dépôt ; elle n'exécute pas un script de vérification fourni dans l'archive.

L'ancienne CLI crée un projet jetable, des captures binaires, un Marker, une
Release et une Layer. Tous ses processus sont terminés avant la copie complète
de sauvegarde, `.layrs` compris. La nouvelle CLI doit retrouver exactement les
records et identités exposés par timeline/markers/releases/layers/status, lire
les octets, passer doctor, restaurer la cible historique, l'annuler puis créer
une nouvelle capture. Les anciennes Operations restent présentes.

L'incident simulé altère un octet du nouveau binaire extrait : la vérification
doit refuser et ce binaire ne doit plus être exécuté. Le projet modifié est
conservé dans `incident-state-preserved` en déplaçant tous les enfants du checkout.
Sa racine reste vide au même chemin et conserve son identité filesystem native ;
la recette n'invente pas une relocation de Worktree. La sauvegarde est copiée vers
cette destination vide sans écrasement, puis l'ancienne CLI vérifie le
snapshot exact et doctor. Elle n'est jamais lancée sur les données post-upgrade.
La nouvelle capture reste dans l'état d'incident conservé mais n'appartient pas au
projet revenu au point de sauvegarde : le rapport le précise explicitement.

Tous les dossiers, archives extraites et `report.json` sont conservés pour revue ;
aucune suppression récursive n'est exécutée par cette recette. Une interruption
laisse sa fixture en place et ne constitue pas un PASS. Conserver ensemble rapport,
deux manifests et SHA256SUMS ; ne pas extrapoler à une paire de versions différente.

Pour Linux depuis Windows, utiliser `node scripts/test-native-upgrade-linux.mjs`
avec les mêmes arguments. L'image locale par défaut est
`layrs-native-packaging:validation` (remplaçable par `LAYRS_UPGRADE_LINUX_IMAGE`) ;
elle doit déjà exister. Le script la résout en ID immuable avant exécution et
conserve cet ID dans `container.json`. Il lance un nouveau conteneur
Debian Bookworm/glibc x64 disposant de Node, `--network none --pull never`, sources
et deux archives montées en lecture seule, sortie de fixture dédiée en écriture.
Aucun volume Cargo ni installation persistante n'est nécessaire : seuls les
exécutables extraits s'exécutent. Le nom/digest de l'image doit figurer dans la
preuve finale. La qualification attend une nouvelle archive issue du gel des
sources ; les archives existantes `qualified2` ne sont pas réétiquetées.

Validation préparatoire : `node --test scripts/native-upgrade-input.test.mjs`
**4tests PASS**, et lecteur validé sur les deux véritables archives qualified2
(22artifacts chacune), sans lancer leurs exécutables.
