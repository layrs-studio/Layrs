# Recettes CI des interfaces natives

`node scripts/test-native-ui-ci.mjs web` crée une fixture PostgreSQL/TLS réelle,
construit Studio Web puis exécute `studio-web-native.spec.ts`. Le frontend et les
API partagent une origine HTTPS du proxy de test. Le proxy vérifie la CA backend,
refuse une origine étrangère et traduit uniquement l'origine locale autorisée.
Chromium fait confiance à la seule clé du certificat jetable ; aucune option
globale `ignoreHTTPSErrors` n'est utilisée.

`node scripts/test-native-ui-ci.mjs desktop` exécute les scénarios SCM et équipe
Tauri/WebView2. La fixture équipe est obligatoire en CI : son absence échoue.
Le scénario historique `desktop-native.spec.ts`, destiné à l'ancien écran Local,
reste conservé et distinct du gate des écrans natifs.

Prérequis : dépendances JS verrouillées, Rust 1.85, Chromium pour Web, PostgreSQL
jetable autorisant CREATE DATABASE/ROLE. Fournir `LAYRS_TEST_DATABASE_URL` avec
`LAYRS_TEST_DATABASE_ISOLATED=1`. Sur Windows sans endpoint explicite, le harness
utilise les binaires PostgreSQL préinstallés du runner, ou `PGBIN`, et crée son
propre cluster local. Une dépendance absente échoue explicitement. La CI
précompile Desktop dans `target/studio-desktop-e2e` afin que le délai de démarrage
ne serve pas à mesurer une compilation à froid. Le worker Lens est construit
explicitement avant les tests Windows qui l'attendent.

Chaque exécution crée un fichier privé de credentials et un registre de ses seuls
DB/rôles. Elle arrête ses processus et nettoie ces ressources même si un test
échoue. Un démarrage interrompu peut être nettoyé avec l'exemple Rust :

```text
native_ui_fixture --cleanup-fixture /chemin/prive/resources.json
```

Le nettoyeur valide les noms de fixture et n'accepte pas de noms de production.
Les credentials restent dans l'environnement et les fichiers locaux, jamais dans
les arguments. `tmp/` et ces fichiers ne sont pas téléversés comme artefacts.
Traces, vidéo et screenshots sont désactivés pour les parcours manipulant des
secrets. Le journal de démarrage reste privé en cas d'échec.

`node scripts/test-native-ui-ci.mjs lifecycle` vérifie uniquement création et
nettoyage, sans navigateur. Exécuté localement avec succès le 20 septembre 2026,
ainsi que les contrôles TLS/origine de l'API du proxy. La collecte du test Web
réussit ; **le navigateur Web CI n'a pas été exécuté dans cette session locale**.
Les preuves Desktop natives locales figurent dans les journaux T1/T5/T8.
`nativeProofSeed` fournit des données de preuve synthétiques via la vraie API ;
il ne remplace pas la recette séparée du runner Docker.
