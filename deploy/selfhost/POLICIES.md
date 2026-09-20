# Changer ou restaurer la politique d’accès d’un projet

Les politiques déterminent les droits sur le Space. La gestion habituelle passe par
les équipes et les rôles individuels. Une racine personnalisée suspend ces
modifications gérées : elles ne doivent jamais effacer implicitement des règles
personnalisées. Les règles de contenu v2 peuvent limiter la lecture à certains
chemins. Les noms de fichiers et la structure restent visibles : ces règles
protègent les octets des fichiers, pas leur existence.

Le changement requiert un rôle propriétaire/administrateur du Workspace et
`policy.administer` selon la politique actuelle. La nouvelle politique ne peut
pas donner au demandeur le droit de s’autoriser lui-même. Elle peut lui retirer
son accès : vérifier la cible et conserver un administrateur autorisé.

## CLI

Depuis un checkout natif lié au projet, avec une session ouverte :

```text
layrs remote policy-management --endpoint https://serveur.exemple
layrs remote policy-preview blake3:IDENTIFIANT_COMPLET --output politique.json --endpoint https://serveur.exemple
layrs remote policy-apply politique.json --endpoint https://serveur.exemple
```

Remplacer la cible par un identifiant complet de PolicySet déjà admis dans ce
Space. `policy-management` retourne `mode`, `currentPolicySetId` et
`managedPolicySetId`. Pour rétablir les équipes et rôles, choisir **exactement**
`managedPolicySetId`, pas une reconstitution des membres ni une ancienne racine
similaire. L’aperçu enregistre le serveur, le compte, le Store/Worktree, le Space,
la racine actuelle, sa révision et la racine proposée. Il n’applique rien.
Le fichier de sortie doit être nouveau ; `--json` permet aussi de lire le résultat.

Après vérification de l’aperçu, `policy-apply` journalise l’intention puis applique
une transaction sans capture, déplacement de tête ni réception implicite. Une
attente devenue obsolète est refusée. Le changement de racine et l’invalidation
des anciennes autorisations se produisent dans la même transaction serveur.
Reproposer la racine déjà active ne crée pas une nouvelle révision des droits.

En cas de réponse perdue, relancer **le même fichier** : le client reprend la
demande et obtient le même reçu. Une publication/réception concurrente est
bloquée tant que le résultat de cette intention administrative est inconnu.
Ne pas effacer les journaux locaux pour contourner cette attente. Aucun jeton
d’accès n’est contenu dans l’aperçu ou le journal d’intention.

## Desktop

Dans **Équipe, protections et intégrations → Accès au projet**, charger la gestion
des accès puis ouvrir **Changer la politique d’accès**. Saisir la cible ou choisir
**Préparer le retour aux équipes et rôles**. Contrôler les identifiants affichés,
puis appliquer explicitement l’aperçu. En cas de réponse perdue, réessayer ce même
aperçu. Recharger les équipes après restauration des rôles gérés.

La surface Web affiche le mode courant ; l’application de la politique avec
journal de reprise est disponible dans la CLI et Desktop. Les parcours sont
qualifiés séparément dans les journaux T4/T8 ; une compilation Web ne constitue
pas une recette navigateur.

## Créer des règles personnalisées

Un fichier JSON de règles v1 ou v2 peut produire un aperçu, sans SQL ni envoi
d’une transaction arbitraire. Le document doit respecter le schéma fermé et
tenir dans 64 Kio ; les champs inconnus, actions inconnues et chemins non
normalisés sont refusés. Exemple v2, à adapter avec l’identifiant réel du compte :

```json
{
  "schemaVersion": "layrs.space-policy-rules.v2",
  "rules": [],
  "contentPolicies": [{
    "principalIds": ["principal:IDENTIFIANT_DU_COMPTE"],
    "default": "deny",
    "rules": [{
      "effect": "allow",
      "selector": { "kind": "subtree", "path": "public" }
    }]
  }]
}
```

Cette politique permet à ce compte de lire les fichiers du sous-arbre `public`
si ses droits généraux existants autorisent déjà leur lecture. Elle n’accorde
aucun rôle supplémentaire. `exact` cible un chemin précis ; `subtree` cible ce
chemin et ses descendants, avec `/` comme frontière. Toute règle `deny` qui
correspond l’emporte sur `allow`. Noms et structure restent visibles.

```text
layrs remote policy-preview-rules regles.json --output apercu.json --endpoint https://serveur.exemple
layrs remote policy-apply apercu.json --endpoint https://serveur.exemple
```

L’aperçu **ajoute une Policy** à la racine courante, sans supprimer ni réécrire
les règles précédentes. Il affiche les règles exactes et les racines avant/après.
Une règle allow supplémentaire ne peut donc annuler un deny existant. Pour
abandonner les règles personnalisées, préparer explicitement le retour à
`managedPolicySetId`. La racine est limitée à 32 Policies.

Dans Desktop, le même panneau propose la saisie du document JSON et sa
préparation. Lire l’aperçu puis appliquer explicitement. L’aperçu reste une
lecture : aucun objet n’est téléversé avant l’application. Celle-ci journalise
aussi l’achèvement du téléversement des règles avant de changer la racine : une
réponse perdue reste récupérable même si la nouvelle politique retire le droit
de téléverser. Réutiliser le même aperçu ; ne pas reconstruire une autre demande.

## Lire un fichier avec des droits limités

Depuis un checkout déjà lié au Space, la CLI peut lire un fichier précis d’une
capture publiée sans modifier le checkout :

```text
layrs remote file blake3:IDENTIFIANT_STEP docs/guide.txt --output copie-guide.txt --endpoint https://serveur.exemple
```

Le chemin est relatif au projet, avec `/` comme séparateur. Le serveur autorise
le couple capture/chemin exact, même si un fichier identique existe ailleurs.
Le client vérifie les objets et le contenu avant de créer la sortie. Le fichier
de sortie doit être nouveau ; aucun fichier existant n’est remplacé. Cette
commande lit un fichier ordinaire, pas un dossier, lien ou conflit non résolu.

Avec des droits limités, le clonage et la réception complète sont refusés avant
toute capture ou application locale. Une ancienne copie complète ne donne pas
de nouveaux droits sur le serveur. Ce parcours ne crée pas de checkout partiel.
Les interfaces de comparaison, revue et CI riches sont également refusées dans
ce mode lorsqu’elles pourraient exposer du contenu non autorisé.

## Conservation et diagnostic

Les anciennes politiques, les événements et les reçus restent immuables. La
sauvegarde PostgreSQL/CAS conserve la provenance permettant de distinguer le
mode géré du mode personnalisé. Si cette provenance manque ou diverge, la
gestion des rôles est refusée plutôt que reconstruite par supposition.
