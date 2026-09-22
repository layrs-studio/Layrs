# Projets partiels et fichiers réservés

Un membre voit le nom et le chemin des fichiers interdits, avec un cadenas.
Le contenu interdit n'est pas téléchargé. Créer ou déplacer un fichier sur cet
emplacement est refusé, y compris via un parent ou un alias de casse Windows.
Un fichier créé par un éditeur externe est conservé ; Layrs signale la collision
et refuse sa capture. L'absence d'un fichier interdit ne devient pas une suppression.

## Définir les droits

Les politiques v3 définissent séparément lecture et écriture par membre.
Écrire exige également la lecture et les droits généraux du projet. Un refus
correspondant dans une politique reste prioritaire. Exemple :

```json
{
  "schemaVersion": "layrs.space-policy-rules.v3",
  "rules": [],
  "pathPolicies": [{
    "principalIds": ["principal:IDENTIFIANT_DU_MEMBRE"],
    "read": {
      "default": "allow",
      "rules": [{"effect": "deny", "selector": {"kind": "exact", "path": "comptabilite/salaires.csv"}}]
    },
    "write": {"default": "allow", "rules": []}
  }]
}
```

Desktop propose le choix du membre et du fichier dans l'éditeur de politique.
Pour modifier une politique existante en CLI, lire ses identifiants et remplacer
sa version exacte ; ajouter une règle `allow` ne supprime pas un ancien `deny`.

```text
layrs remote policy-read --endpoint https://serveur.exemple
layrs remote policy-replace policy:IDENTIFIANT regles.json --output apercu.json --endpoint https://serveur.exemple
layrs remote policy-apply apercu.json --endpoint https://serveur.exemple
```

## Ouvrir et travailler

Choisir une capture publiée et un dossier existant vide :

```text
layrs remote clone-partial space:IDENTIFIANT blake3:IDENTIFIANT_STEP dossier-vide --endpoint https://serveur.exemple
```

Depuis ce nouveau dossier, `partial status` donne la génération locale et la
base distante. Modifier un fichier autorisé puis capturer et publier :

```text
layrs partial status
layrs partial capture GENERATION
layrs remote publish-partial NUMERO_CAPTURE layer:IDENTIFIANT --endpoint https://serveur.exemple
layrs remote receive-partial layer:IDENTIFIANT --endpoint https://serveur.exemple
layrs partial history
layrs partial fsck
```

Desktop fournit capture, publication, réception, création de fichier vide et
déplacement/renommage de fichier. Une création exige une vérification serveur
de la destination, même si aucun fichier n'y apparaît. Les droits sont contrôlés
de nouveau lors de l'application et de la publication. Les captures conservent
leurs octets : publier une capture ne remplace pas une édition locale ultérieure.

Une publication interrompue se reprend avec le **même numéro de capture**.
Un reçu serveur appliqué et une acceptation locale encore en attente sont deux
états distincts. Ne pas supprimer `.layrs` ou ses journaux pour débloquer le projet.
Après une interruption locale, utiliser la reprise correspondant au diagnostic :

```text
layrs partial resume-installation
layrs partial resume-namespace
layrs partial resume-refresh
layrs partial resume-restore
```

## Restaurer, annuler et rétablir

Desktop affiche les captures locales et permet de restaurer les chemins choisis,
puis d'annuler la restauration. Les autres fichiers restent inchangés. Un aperçu
est recalculé avec les droits actuels ; une révocation entre aperçu et application
refuse l'opération. Les octets remplacés restent récupérables.

En CLI, préparer un fichier `demande.json` contenant la génération affichée par
`partial status`, le numéro de capture et les chemins à restaurer :

```json
{"expectedGeneration": 3, "source": {"kind": "capture", "sequence": 2, "paths": ["src/main.rs"]}}
```

```text
layrs remote partial-restore-preview demande.json --output restauration.json --endpoint https://serveur.exemple
layrs remote partial-restore-apply restauration.json --endpoint https://serveur.exemple
```

Pour annuler, préparer un nouvel aperçu avec `source` égal à
`{"kind":"inverse","operationId":"IDENTIFIANT_RETOURNE"}` et la génération
courante. Inverser cette annulation permet de rétablir l'opération. Restaurer ne
publie rien automatiquement : capturer/publier reste un choix explicite.

## Réception, révocation et migration

La réception refuse d'écraser une édition locale divergente. Après réception
d'une révocation, une ancienne copie propre est déplacée dans la zone privée
`.layrs/projection-quarantine`, avec un journal de récupération. Cette conservation
ne prétend pas effacer les copies déjà obtenues. Une réautorisation télécharge
à nouveau le contenu autorisé et vérifié.

Pour passer d'un checkout complet à un checkout partiel, sélectionner une Layer
avec une tête locale et distante identique, sans modification non capturée :

```text
layrs remote migrate-partial layer:IDENTIFIANT nouveau-dossier-vide --endpoint https://serveur.exemple
```

La migration crée un dossier distinct ; le checkout complet d'origine conserve
toutes ses Operations et son historique. Elle ne convertit pas les anciens objets
en structures incomplètes. Le nouveau format3 est refusé par les lecteurs format2.
Garder la source jusqu'à vérification du nouveau dossier et des sauvegardes.

## Périmètre

Le transport de publication partielle accepte actuellement les fichiers ordinaires,
au plus 1024 chemins et 16 Mio d'octets remplacés par capture. Les lectures et la
réception utilisent le transfert vérifié en flux, borné à 1 Gio par fichier et
4 Gio cumulés. Les liens et conflits structurels restent visibles et réservés,
sans matérialisation simulée. Windows refuse le mode exécutable non représentable.
Le déplacement de dossier et la transformation d'un dossier existant en fichier
ne font pas partie du parcours local actuel. Les protections de Layer restent
actives ; un refus de Gate ne doit pas être contourné par les patches partiels.
