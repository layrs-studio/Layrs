# Créer un projet et utiliser ses lenses

Ce guide décrit Studio Web et Desktop dans les sources actuelles. Une lens est
installée **une fois sur le serveur**, puis choisie **par projet**. Les clients
récupèrent les composants d'affichage de ce projet automatiquement.

## Créer le projet

1. Connectez-vous dans Studio Web, ouvrez **Projets**, puis **Nouveau projet**.
2. Indiquez le nom et l'identifiant du projet. Sans option serveur activée,
   le projet appartient directement à l'entreprise. Si plusieurs espaces sont
   disponibles, choisissez d'abord le contexte dans le sélecteur **Organisation**.
   **Projets personnels** apparaît uniquement lorsque le serveur autorise les projets
   personnels ; les organisations supplémentaires nécessitent leur propre option.
3. Choisissez les versions de lenses proposées. Les versions intégrées sont
   présélectionnées ; les extensions peuvent être associées explicitement
   lorsqu'elles correspondent à plusieurs lenses.
4. Cliquez sur **Créer le projet**. Le projet s'ouvre avec un Layer initial
   vide. Connectez ensuite un dossier depuis Desktop pour publier ses fichiers.

Dans Desktop, choisissez **Nouveau projet → Récupérer un projet de l'équipe**,
sélectionnez le projet et un dossier local vide, puis **Récupérer le projet**.
Cela fonctionne aussi avant toute première capture côté serveur. Vous pouvez
ensuite ajouter vos fichiers, les capturer et les publier depuis
**Synchronisation**. La récupération seule ne publie rien sur le serveur.

Si le dossier choisi n'est pas vide, Desktop vérifie son projet avant toute action :

- Même projet : **Lier et ouvrir ce projet** utilise la copie existante sans remplacer ses fichiers.
- Autre projet accessible sur ce serveur : **Synchroniser ce projet local** ouvre sa synchronisation.
- Projet local absent du serveur : **Préparer la publication de ce projet** ouvre le choix du contexte et de l'identifiant. Confirmez **Créer ce projet sur le serveur**, puis **Publier** pour envoyer les captures.
- Fichiers ordinaires, données Layrs invalides, connexion incompatible ou accès insuffisant : le clonage est refusé, les fichiers sont conservés.

La vérification est répétée au moment de l'action. Un projet inaccessible n'est
pas considéré comme un projet absent pouvant être publié.

Dans **Changements**, **En cours** compare le dossier à sa capture la plus récente,
automatique ou manuelle. Lorsque la capture automatique enregistre les fichiers,
ils restent visibles sous **Dernière capture**, avec le badge **Automatique**.
Une capture reçue sans capture locale est indiquée comme **Capture de référence**.
L'historique conserve les captures précédentes ; **Versions** sert à nommer
les étapes importantes. Capturer et publier sont deux opérations distinctes.

Si la réponse réseau est perdue, renvoyez **les mêmes valeurs** : le serveur
retrouve le projet créé. Un identifiant déjà pris avec d'autres paramètres est
refusé. Créer le projet ne publie pas les fichiers du dossier local.

Les deux options serveur sont indépendantes et désactivées par défaut :
`LAYRS_MULTI_ORGANISATION_ENABLED` et `LAYRS_PERSONAL_PROJECTS_ENABLED`.
La création d'une organisation est une action séparée réservée à l'administration
de l'installation. Elle n'est jamais une étape obligatoire du formulaire projet.
Elle se trouve en dernière position dans le menu **Organisation**. Ce sélecteur
disparaît quand le mode multi-organisation est désactivé.

Dans **Administration → Organisations**, l'administrateur peut activer le mode
multi ou revenir au mode mono. Pour revenir au mode mono, il choisit l'organisation
principale, lit la liste des organisations qui deviendront inaccessibles et recopie
le nom de la principale pour confirmer. Les autres projets restent conservés ;
réactiver le mode multi restaure leurs accès selon les permissions existantes.

Un projet personnel reste réservé à son propriétaire. Un projet d'entreprise
conserve ses permissions propres : être membre de l'entreprise n'ouvre pas tous
ses projets. Les équipes de l'organisation peuvent être affectées à plusieurs
projets, avec un rôle choisi pour chacun. Les équipes historiques limitées à un
projet restent distinctes et conservent leurs droits existants.

Desktop utilise les mêmes contextes pour les projets du serveur. Les dossiers
uniquement locaux restent disponibles séparément ; changer de contexte ne
supprime aucun dossier. Pour le modèle et la migration des installations
existantes, voir le [contrat des contextes](../contracts/project-contexts.md).

## Installer une lens sur l'instance

Un administrateur d'instance ouvre **Administration → Lenses**, choisit un
fichier `.layrslens.json`, lit le nom, la version et le nombre de composants,
puis lance **Vérifier et installer**. Le serveur vérifie manifeste, empreintes,
compatibilité et, si présente, signature. Pour un paquet non signé, il faut
autoriser expressément sa provenance avant installation.

Le dépôt fournit un [paquet Markdown d’exemple](../../deploy/lenses/example-markdown.layrslens.json)
non signé pour essayer ce parcours. Après installation, activez-le dans un
projet et associez-lui les extensions `md` et `markdown`.

La liste indique les versions disponibles et le nombre de projets qui les
utilisent. **Retirer du catalogue** empêche de nouveaux projets de choisir la
version tout en laissant les projets déjà équipés l'utiliser. **Révoquer**
empêche les nouveaux téléchargements et exécutions ; utilisez cette action si
la version ne doit plus être exécutée. Les références historiques restent
conservées.

## Choisir les lenses d'un projet

Dans **Réglages du projet → Lenses**, cochez une version par lens, puis
enregistrez. Vous pouvez préciser les associations sous la forme
`extension=identifiant.lens`, une par ligne, par exemple
`md=example.markdown`. Sans association, le client utilise les extensions
déclarées ; si plusieurs lenses correspondent, l'affichage de secours s'applique.
Une mise à jour installée sur le serveur ne change pas automatiquement les
versions de votre projet. Si la configuration a changé pendant votre édition,
rechargez-la avant de réessayer.

Web et Desktop vérifient les empreintes avant de charger un viewer dans un
worker isolé. Desktop peut réutiliser les versions déjà téléchargées hors
ligne. Après reconnexion, il actualise les choix et les révocations. Une lens
absente, incompatible ou en erreur laisse l'accès aux données autorisées avec
un affichage de secours.

Les aperçus de fichiers publiés affichent aujourd'hui le texte UTF-8 jusqu'à
1 Mio. Les images binaires, gros fichiers et formats sans viewer utilisable
affichent un message de repli. Le droit de lire le chemin est contrôlé par le
serveur avant que son contenu soit transmis.

Pour les détails de format, signature, limites et API, voir le
[contrat de distribution](../contracts/lens-distribution-v1.md).
