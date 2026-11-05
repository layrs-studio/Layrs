# ADR 0018 — Noms et correction des points immuables

Statut : accepté pour le parcours natif PME, 20 septembre 2026.

Un nom est un libellé de recherche, jamais une identité. MarkerId et ReleaseId
sont propres à un objet immuable et à son Space ; aucune correction ne déplace
la cible d'un identifiant déjà communiqué.

Les libellés et versions sont comparés exactement, avec distinction de casse,
sans normalisation Unicode implicite. Le contrat canonique admet 1 à 255
caractères Unicode pour le nom/version ; il n'impose pas SemVer.

Plusieurs Markers visibles peuvent porter le même nom. Le sélecteur par nom
refuse alors l'ambiguïté ; l'identifiant exact reste utilisable. Pour remplacer
un Marker : créer le nouveau point, vérifier sa cible, puis masquer l'ancien
par son identifiant. Masquer ne supprime ni l'objet ni sa cible. `marker show ID`
rétablit sa visibilité dans une nouvelle Operation, sans capturer ou remplacer
le travail courant. Les Views historiques conservent leurs anciennes références.

La création locale d'une Release refuse une version déjà visible. Pour corriger
`v1`, créer par exemple `v1.1` avec des notes explicites ; les deux objets gardent
leurs cibles. Il n'existe pas de renommage ou remplacement en place. Une Release
publiée incorrecte est retirée par un événement de révocation distinct, puis la
nouvelle version est publiée avec ses propres contrôles de droits et preuves.
Révoquer sa publication ne masque ni ne détruit son manifeste. Le produit ne
propose pas de suppression de Release ; restaurer une ancienne View peut changer
la sélection visible mais n'annule jamais un événement de publication distant.

La règle d'unicité concerne la création dans la View locale courante, pas un
registre global de noms. Des objets importés ou reçus peuvent avoir un même
libellé : le sélecteur refuse l'ambiguïté, sans choisir par date ou ordre SQL.
Le nom seul ne suffit donc jamais à autoriser publication ou révocation.

Les entrées invalides et versions déjà visibles sont refusées avant la capture
automatique. Une création valide capture le travail courant, puis accepte le
nouvel objet et la View par comparaison de l'état attendu. Les règles de rétention
de T8 incluent les références historiques ; cette décision n'atteste pas une
implémentation de GC.

Preuves : deux tests `native_references` (ambiguïté, hide/show, correction par
nouvelle version, réouverture, fsck, refus sans capture). Publication/révocation
immutable : recette HTTPS/PostgreSQL du journal T3 et ADR 0016.
