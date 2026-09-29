# ADR 0017 — Identité déclarée locale et recherche des observations

Statut : accepté, 20 septembre 2026. Complément aux ADR0006/0008 ; aucune modification de l’identité des Steps.

## Autorité locale explicite

Un utilisateur configure un ActorId, éventuellement un DeviceId et un nom affichable. Le Store conserve un registre canonique versionné `metadata/local-identities.json`, lié exactement à son StoreId. L’enregistrement est explicite, sous capability exclusive du Store ; un remplacement atomique durable publie la nouvelle configuration. Les lectures refusent liens symboliques/reparse, encodage non canonique, mauvais Store/version et limites dépassées (1024 acteurs/appareils, 64 Kio). Aucun secret n’y figure.

Ce registre constitue une autorité **déclarative locale**, pas une preuve d’identité distante. L’admission locale peut y résoudre les acteurs/appareils enregistrés ; une simple mention d’un ID inconnu dans une observation ne l’enregistre pas. Les autorités antérieures des enregistrements durables restent lisibles. Les anciens IDs restent enregistrés après changement de configuration : une capture déjà préparée peut être acceptée avec son identité figée. Aucune observation ni Operation existante n’est réécrite.

Les captures manuelles et automatiques consultent l’identité active pendant leur préparation sous verrou de lecture. Le batch conserve l’identité et la date explicites de cette préparation pour les retries. `MutationStamp` reste compatible ; ses IDs et sa date ne dépendent pas de la configuration. Initialisation et opérations autres que capture restent sans attribution automatique dans ce lot. Le registre est un fichier additionnel explicite du format natif, sans mutation du schéma SQLite2 ; une sauvegarde locale intégrale doit le conserver. L’absence de fichier signifie aucune identité configurée, jamais une identité devinée. La synchronisation ne copie pas cette configuration locale ni ne la confond avec le principal TLS.

## Lecture reconstruisible

Une projection mémoire groupe les StepObservations validées par Step, dans un Space exact. Elle est reconstruite depuis les autorités à chaque requête ; aucune projection persistante ne peut supprimer une ligne autoritaire ou déplacer une référence. Limites : 100000 observations, 64 Mio de payloads, et 100000 Steps parcourus. Ce premier index ne promet pas encore une accélération persistante de grands historiques.

Un curseur conserve le Step racine, chemin exact, filtre et snapshot des observations : SpaceId, borne d’insertion SQLite locale, nombre et empreinte canonique de toutes les lignes retenues. Les append ultérieurs sont exclus, même pour un ancien Step/Operation et même antidatés. Les lectures suivantes reconstruisent et vérifient ce snapshot ; modification, disparition ou renumérotation des lignes le rend invalide et oblige explicitement à recommencer. La borne n’est pas une chronologie des Steps ni un curseur portable entre Stores. Les anciens curseurs dépourvus de snapshot doivent redémarrer.

Acteur exact et bornes RFC3339 inclusives doivent correspondre à **la même observation**. Un Step sans observation reste visible sans filtre et ne correspond pas à un filtre d’acteur/période. Toutes les observations du Step dans le snapshot sont retournées pour affichage, sans choisir arbitrairement un auteur. Les timestamps sont comparés en instants ; aucune continuité à travers un renommage n’est inférée. La portée actuelle est locale : les observations non répliquées depuis le serveur ne sont pas inventées.
