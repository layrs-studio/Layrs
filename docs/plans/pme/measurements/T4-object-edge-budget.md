# Budget d'admission des arbres larges

La recette HTTPS 99 000 chemins de root a échoué avant le réseau lors de
l'initialisation locale : `AdmissionBudgetExceeded`, `object graph edges`, limite
16 384. Cette garde utilisait le même plafond que les parents des DAG Step et
Operation, alors que chaque entrée de Tree consomme une référence même quand de
nombreux chemins partagent le même FileId.

Correction bornée : `MAX_OBJECT_DEPENDENCIES = 262_144` pour la somme des références
des objets du batch, cohérente avec le budget existant de parcours des fermetures.
Parents de Step/Operation et nombre de nœuds restent à 16 384. Objets/entités du
batch restent à 16 384, admissions à 65 536 et mutations à 4 096. Les plafonds CAS,
de taille de fichier et de visite ne changent pas. Une fermeture agrégée peut donc
encore être refusée même si son Tree seul passe la prévalidation.

La prévalidation statique est aussi exécutée avant toute préparation CAS dans
`NativeScmRepository::apply_batch`. Auparavant ces budgets n'étaient évalués
qu'après écritures CAS ; un refus pouvait laisser des objets non référencés.
L'autorisation et la validation complète restent dans la transaction native.

Tests Windows `scm_admission budgets::` : **5 PASS34,94s**. Tree de 99 000 entrées
réellement admis et relu ; Tree de 262 145 références refusé avec inventaire complet
SQLite/CAS identique, sans nouveau sidecar ; 16 900 parents Step toujours refusés
au plafond 16 384 et batch de 16 385 objets toujours refusé. Le jeu utilise des
contenus répétés, pas 99 000 objets uniques. Cela valide cette frontière précise ;
la recette réseau 99k doit encore être relancée et peut révéler une autre borne.

Sources : [admission.rs](../../../../crates/layrs-store-local/src/scm_v1/admission.rs),
[repository.rs](../../../../crates/layrs-store-local/src/scm_v1/repository.rs),
[tests](../../../../crates/layrs-store-local/tests/scm_admission/budgets.rs).

Régression complète après correction : **23 tests scm_admission PASS47,27s**,
dont isolation des Spaces, fermeture complète, corruptions et frontières
d'Operations. Clippy ciblé strict Windows **PASS13,30s**.
