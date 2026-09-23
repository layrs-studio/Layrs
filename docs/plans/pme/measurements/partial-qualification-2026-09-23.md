# Qualification des extensions partielles — 23 septembre 2026

Sources de production figées : `fbd8d6f49858605919451e5cbfe8656b96e66d42`.
Les trois paquets ont été construits avec un arbre Git propre. Les journaux
ajoutés ensuite ne changent pas ces binaires. Détails, manifestes complets,
empreintes et résultats de distribution dans
[partial-artifacts-2026-09-23.json](partial-artifacts-2026-09-23.json).

**Complément courant Desktop :** commit `7ecae36`, choix de conflits persistants
sur ce poste, 5 tests Node et Desktop réel43,9sPASS (conflit, sélection, page
rechargée, choix retrouvé et appliqué). MSI **qualified7** construit et vérifié,
non signé/non installé. Les archives natives restent **qualified6**, leurs
sources inchangées ont été revérifiées.
[Manifeste et périmètre du complément](partial-desktop-followup-2026-09-23.json).
Le reste du rapport conserve les preuves du lot `fbd8d6f`.

**Mesure mémoire complémentaire :** scénario HTTPS partiel1Gio1PASS,270,49s ;
pic résident63,8Mio, privé échantillonné27,1Mio, 4 316 observations. Processus
clients+serveur intégré, PostgreSQL/cache OS/enfants exclus ; debug, octets
répétés et compilation MSI concurrente. Attente nominale50ms, moyenne observée
62,7ms ; pics privés entre observations possibles. Ce résultat ne calibre pas
le débit et ne prouve pas une borne universelle.
[Mesure, commande et empreintes des logs](partial-sync-memory-2026-09-23.json).

## Comportements vérifiés

| Contrôle | Résultat | Portée |
| --- | --- | --- |
| HTTPS/PostgreSQL, 100 Mio | PASS, 121,58 s | Publication en blocs, reprise, candidat/revue/intégration, dossiers, fusion et historique |
| Même recette, 1 Gio | PASS, 284,71 s | Fichier exact, réponse perdue, réouverture/rejeu, édition ultérieure conservée |
| Desktop réel | PASS, 53,3 s | Droits/réservations, publication, revue approuvée, intégration, brouillon de fusion, dossier, restauration historique et fsck |
| Projection locale Windows / Linux | 73 / 74 PASS | Stockage, réservations, capture, restauration, namespace et rebase |
| Rebase après correction de course | 11 PASS par OS | Dernière vérification après le point d'injection, collision externe conservée, reprise refusée sans écrasement |
| Client natif / modèle UI | 47 / 12 PASS | Contrats et opérations locales |
| TypeScript | PASS | UI, Web et Desktop |
| Clippy / format Rust | PASS | Workspace, tous targets, avertissements refusés ; format vérifié |
| Migrations immuables | 19 PASS | Migrations append-only et empreintes |

La recette Desktop utilise le vrai backend. Une seconde identité approuve la
revue avant son intégration. Elle ne constitue pas un pilote humain. Les essais
initiaux ayant trouvé une double lecture sous StrictMode et une intégration
activée trop tôt ont été corrigés avant ce résultat final.

Logs locaux conservés dans `D:/Layrs/tmp/` :
`scoped-1gib-20260923.log`, `partial-ui-pass-20260923.log`,
`rebase-linux-20260923.log`, `rebase-linux-final-20260923.log`,
`finish-client-20260923.log`, `finish-ui-model-20260923.log`,
`finish-clippy-final-20260923.log`.

## Distribution qualified6

| Artefact | Résultat |
| --- | --- |
| Archive Windows x86_64 release | 26 fichiers vérifiés, 8 parcours smoke PASS |
| Archive Linux x86_64 release | 26 fichiers vérifiés, 8 parcours smoke PASS, conteneur sans réseau |
| Mise à jour qualified5 → qualified6 puis retour arrière | PASS Windows et Linux, 5 opérations historiques conservées, archive altérée refusée avant exécution |
| MSI Windows | Construction et SHA256 PASS ; non signé, non installé |

Emplacements sous `dist/` :

- `native-windows-qualified6-20260923/layrs-fbd8d6f49858-x86_64-pc-windows-msvc-release.tar.gz`
- `native-linux-qualified6-20260923/candidate/layrs-fbd8d6f49858-x86_64-unknown-linux-gnu-release.tar.gz`
- `desktop-windows-qualified6-20260923/Layrs Studio_0.0.0_x64_en-US.msi`

Les deux archives ont la même empreinte des sources Rust/Cargo :
`cc55b95ec6bfd8ed3cbcf4289e5ad8b513f0a48de770a6d02a4d3736bac790fa`.
Le MSI de 12 300 288 octets a pour SHA256 :
`b2f307dd3b8ebfe3b40c69cf0e37e8b53c0de611291d498b9ccbf1e4503dce91`.
Les contrôles de sommes et de tailles ont été refaits lors de la génération
du rapport JSON. Les archives qualified5 restent des artefacts historiques.

Le retour arrière d'archive porte sur le Store complet FORMAT2. Il ne prouve
ni un retour de schéma Store3 5→3/4, ni une migration PostgreSQL inverse,
ni une mise à jour MSI. La recette Linux utilise un volume Linux natif dédié,
avec provenance du conteneur et rapport exportés dans le JSON.

## Limites conservées

L'utilisateur ne dispose que du poste actuel. Windows hôte et Linux Docker ont
été utilisés ; aucune qualification macOS/ARM, Windows vierge, disque hôte
réellement plein, CI distante, pilote de 3–5 personnes ou opérateur indépendant
n'est ajoutée par ce rapport.

Le test 1 Gio utilise du contenu répété. Il ne mesure ni un lot de 4 Gio,
ni un débit de production calibré. La mesure mémoire complémentaire ci-dessus
porte sur le processus du scénario entier, pas seulement une phase réseau.
La fusion partielle est limitée aux fichiers ordinaires avec une seule tête
distante ; transformations structurelles, liens et bases multiples restent
ouverts. La restauration historique sélectionne les chemins connus et
autorisés actuellement ; elle ne recrée pas un chemin historique inconnu.
Les variantes du brouillon sont durables. Les choix du formulaire étaient
volatils dans qualified6 ; le complément Desktop qualified7 les conserve dans
le stockage local du WebView, sans remplacer la revalidation native.

Des revues critiques indépendantes ont trouvé des défauts corrigés dans les
lots stockage/dossiers et transport/revues. La dernière relecture indépendante
du résultat intégré n'a pas pu être relancée : plafond de threads d'agents
atteint. La relecture racine et les régressions finales ne sont pas présentées
comme une vérification indépendante.

État des travaux et ordre de reprise : [REMAINING](../REMAINING.md),
[REPRISE](../REPRISE.md) et [journal actif](../journal/FINISH-20260923.md).
