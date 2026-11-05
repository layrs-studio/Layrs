# ADR 0023 — ACL de contenu par chemin, phase conservatrice

Statut : accepté et phase conservatrice implémentée, 20 septembre 2026. Complète ADR0012/0022. Aucune migration PostgreSQL ; aucune projection partielle de Worktree.

Les règles Evidence `layrs.space-policy-rules.v2` conservent les règles générales v1 et ajoutent des politiques de contenu par Principal authentifié : défaut allow/deny explicite, sélecteurs exact/subtree sur RepoPath normalisés. Aucun glob, regex, case-folding ou règle Layer. Deny prioritaire entre toutes les politiques ; une règle de contenu ne crée jamais un droit général absent. V1 reste inchangée.

Un Principal auquel s'applique une politique de contenu est restricted même si elle autorise tous les chemins présents. Son Bootstrap ajoute uniquement `contentAccess:{schemaVersion:"layrs.content-access.v1",mode:"restricted"}`. Champ absent pour les comptes sans restriction ; anciens lecteurs stricts refusent le champ inconnu plutôt que d'installer un checkout incomplet.

Chaque lecture restreinte de fichier nomme une Step admise (ou LayerHead courant résolu exactement), son Tree racine, RepoPath et FileId/objet. Le serveur parcourt les Trees canoniques exacts, sans raccourci Product/Conflict. Manifestes et chunks doivent appartenir au fichier du chemin. La capacité lie claim, identité et epoch ; chaque GET/Range revalide. Structure, noms et identifiants restent visibles ; jamais de faux Tree filtré.

Phase initiale : aucun objet opaque Product/Evidence/Conflict, détail de collaboration/CI ni comparaison complète pour les Principals restricted. Ces routes refusent globalement jusqu'à qualification d'une provenance plus fine. Mutations de contenu, références, droits et revues refusées. Un administrateur non restreint reste nécessaire pour gérer la politique ; pas d'exception implicite rétablissant des droits retirés.

Clone/receive complets refusent restricted avant capture, raccourci inchangé ou mutation locale. Le cache d'un FileId public ne contourne pas le refus pour son alias privé. Une révocation n'efface pas des fichiers déjà téléchargés. Des octets également publiés sous un chemin autorisé restent lisibles par ce chemin : protection de l'accès, pas propriété globale de secret des octets.

Preuves requises : v1 inchangée ; exact/subtree/default/deny ; aliases FileId/chunks ; faux chemin/root/Space ; opaque/CI ; anciennes capacités/Range révoqués ; écritures refusées sans mutation ; clone/receive/cache sans Operation/fichiers modifiés. JournalT4 conserve l'état réel ; le critère de projection partielle reste ouvert.
