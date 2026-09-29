# Restauration historique partielle — 23 septembre 2026

Implémenté : restauration d'un Step publié sous les droits actuels. La sélection
vide couvre toutes les feuilles ordinaires actuellement modifiables et connues,
y compris les absences observées. Une sélection explicite laisse les autres
modifications locales intactes. Un chemin réservé, non connu, non régulier ou
sans autorisation actuelle est refusé. Les fichiers absents dans l'ancien Step
sont restaurés comme suppressions, sans confondre absence et contenu interdit.

L'ancienne version est relue par chemin exact. Les octets sont vérifiés avant
l'application via le journal de rebase ; aucune requête réseau sous son verrou.
La base distante courante reste inchangée et le résultat devient une nouvelle
capture. Le brouillon et le résultat achevé sont rejouables. Les octets remplacés
restent conservés par le journal, avec vérification des identités avant commit.

CLI : `remote partial-historical-preview CAPTURE LAYER STEP PATHS.json --output
PREVIEW.json`, puis `remote partial-rebase-apply PREVIEW.json CHOICES.json`.
PATHS est une liste triée, éventuellement vide ; CHOICES vaut `{}`.
Desktop : formulaire « Restaurer une version publiée » puis aperçu/application.

Validation HTTPS/PostgreSQL : recette complète1PASS,0ignoré,121,58s dans
`D:/Layrs/tmp/historical-https-20260923.log`. Sélection interdite refusée,
restauration de toutes les feuilles modifiables, base préservée, rejeu exact et
publication. Clippy client/store/CLI et TypeScript PASS. Validation Desktop réelle
53,3sPASS : sélection ciblée et conservation d'un déplacement parallèle.

Limites : les chemins historiques inconnus de la projection courante ne sont
pas créés automatiquement ; ils exigent une observation de destination actuelle.
Symlinks et transformations dossier/fichier restent hors de cette opération.
