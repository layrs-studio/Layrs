# ADR 0022 — PolicyRoot v1 et provenance des politiques gérées

Statut : accepté, 20 septembre 2026. Complète ADR 0011/0012 ; ne définit aucune ACL partielle de contenu.

## Autorité et admission

La mutation Sync `policyRootMutation` compare exactement la racine et l'epoch attendues. Son auteur doit posséder `transaction.apply`, `policy.administer` **sous l'ancienne racine**, ainsi qu'un rôle Workspace owner/administrator actif, revérifié sous verrou. Une politique proposée ne peut autoriser sa propre admission ni ses mutations sœurs.

Le premier lot admet les règles `layrs.space-policy-rules.v1` : Principals et actions explicites, allow/deny, refus prioritaire et absence de grant par défaut. Elles s'appliquent à tout le Space, y compris aux demandes d'objets sans chemin. L'extension v2 de l'ADR0023 ajoute les restrictions de contenu ; v1 ne reçoit aucune sémantique implicite par chemin. Champs inconnus, schémas futurs et wildcards sont refusés. L'auteur peut explicitement retirer ses propres droits ; aucun privilège de restauration implicite ne les contourne.

Les octets canoniques de chaque Policy, PolicySet et Evidence de règles sont vérifiés avec leur type, empreinte, Space et fermeture exacte. Une version logique de Policy ne peut désigner deux objets différents. Dans ce lot, les Policies ont des dépendances Step/Product et un prédécesseur vides ; le PolicySet référence exactement l'Evidence de règles déclarée. Bornes : 32 Policies, 1 Mio par document, 8 Mio agrégés de Products.

L'admission d'un Product Policy seul demeure inerte. L'activation, ses objets, les éventuelles Steps/View sœurs, le snapshot d'autorisation, l'epoch, l'événement `policy.root_changed` et le reçu sont engagés atomiquement. Une racine inchangée est un no-op sans nouvelle epoch. Un rejeu exact restitue le reçu initial, même après changement d'epoch ; une nouvelle requête périmée est refusée.

## Coexistence avec les APIs de gestion

Teams/grants/invitations construisent une politique gérée depuis leurs propres projections. Ils ne doivent jamais effacer une politique personnalisée. Le serveur retrouve la dernière transition gérée dans `policy_root_history`, en distinguant les transitions Sync au moyen de leur reçu canonique, de son empreinte et de l'événement correspondant. Toute incohérence ou ambiguïté est refusée. La recherche est bornée à 1 000 transitions.

Le mode est `managed` uniquement si l'identité de la racine courante égale celle de cette dernière racine gérée ; des règles ressemblantes ne suffisent pas. En mode `custom`, les mutations gérées renvoient `409 custom_policy_root_active`, y compris création/acceptation d'invitations lorsqu'elles installeraient des droits. Refuser/révoquer une invitation ne crée pas de grant. Restaurer explicitement **l'identité exacte** de la dernière racine gérée réactive ces APIs. Aucune projection de membres n'est reconstruite à partir de règles arbitraires. Aucune migration n'est nécessaire.

## Parcours client

`preview_stored_policy(workspace, id)` lit l'autorité fraîche sans modifier le binding ou ses Operations. L'aperçu sérialisable contient origine, Principal, Space, Store, Worktree, attentes et requête complète avec digest. `apply_policy_preview` vérifie ces liens puis persiste l'intention et le pending avant le POST. Un résultat réseau incertain conserve cette requête exacte ; une autre publication ou cible ne peut la remplacer. Un reçu terminal vérifié libère le pending. La réception de la View reste une action distincte.

Le journal privé est borné à 256 Kio, écrit dans un temporaire exclusif, synchronisé avant remplacement et suivi d'une synchronisation du répertoire sur Unix. Les liens et reparse points détectables sur le chemin sont refusés ; ceci ne prétend pas isoler le processus contre un attaquant local capable de remplacer concurremment tous ses répertoires. Aucun token n'est enregistré dans l'aperçu.

`preview_policy_rules(workspace, bytes)` accepte uniquement le document fermé v1/v2, borné à 64 Kio, validé par le même type `NativePolicyRules` que l'autoriseur. Il construit un aperçu `layrs.policy-root-preview.v2` avec une Evidence canonique et une Policy nouvelle ajoutée à la racine exacte observée. Toutes les Policies précédentes sont conservées ; au plus 32. L'aperçu lie la racine de base, les règles, le Product et la requête par leurs empreintes ; ce n'est pas un import de SyncTransaction arbitraire. L'aperçu seul n'écrit ni objet ni pending. La CLI expose `policy-preview-rules FICHIER --output APERCU`, puis `policy-apply APERCU`.

L'application écrit le pending avant le réseau, téléverse l'Evidence puis persiste son achèvement **avant** le POST de mutation. Ce marqueur évite qu'un rejeu après réponse perdue exige un droit de téléversement que la nouvelle politique a retiré. Le reçu mémorisé localement peut seulement éviter un téléversement redondant ; le POST exact retourne toujours le reçu faisant autorité au serveur. Les anciens journaux d'aperçu v1 restent lisibles.

## Preuves et limites

PG + backup/restore : PASS 70,02 s, y compris auto-autorisation refusée, enforcement des objets sans chemin/revues/historique/transactions, CAS concurrent, rollback de Step sœur, no-op/rejeu, type/Space/dépendances/corruption refusés et provenance personnalisée restaurée. Client HTTPS : PASS 32,24 s, aperçu sans mutation, isolement compte/Store/Worktree, réponse perdue après commit, pending récupéré à l'ancienne epoch, restauration gérée et refus périmé rejoué. Test Windows du remplacement d'intention existante et des bornes : PASS.

Extension opérateur JSON v1/v2 : client HTTPS/PostgreSQL PASS34,07s. Documents fermés, base/empreintes falsifiées refusées ; preview sans effet ; upload canonique puis perte de réponse après retrait du droit d'upload ; reprise/rejeu exacts ; restriction v2 et restauration gérée vérifiées. Les assertions de ce lot ne constituent pas une nouvelle recette backup ou navigateur.

Les ACL partielles de contenu nécessitent un autre contrat couvrant fermeture des objets, fetch/download, historique, comparaison et collaboration. Ce lot ne les simule pas par des filtres d'interface.
