# ai-work-supervision — PRD

Maturité du document : proposition

Maturité : prototype

Provenance : ce document reprend la spécification de mission `specs/ai-work-supervision/prd.md`
de l'analyse du 2026-10-14. Seuls changements : la ligne qui déclare le niveau de ce texte (ci-dessus), la
section 11 (paquets de travail) et des réécritures de forme exigées par la norme des paquets de
travail de `libre-ai/project-governance` (`docs/specifications/WORK-PACKAGES.md`) : chaque
paramètre écrit entre chevrons y est écrit en capitales (`HARNESS`, `JOURNAL`, `ORG/DEPOT`). Une
correction s'y ajoute : le rattachement de P15 à `libre-ai/signalement` (ADR-0055 D2, D94).
`cahier-des-charges.md` est à côté de ce fichier ; `architecture.md`, `tests-et-evaluation.md`,
`questions-ouvertes.md`, `CADRAGE.md`, `HANDOFF-phase-0.md`, `clones.json` et `inventaire/`
désignent l'analyse de mission et ne sont pas dans ce dépôt.

Portée du niveau : « prototype » qualifie le projet observé (cœur v0 exécuté au faux agent, B′
`pending`) ; ce document est une proposition. La fiche porte `maturity: idea`
(`project.v1.yaml:27`) : autre échelle, écart consigné (`cahier-des-charges.md` en-tête).

Projet : `libre-ai/ai-work-supervision`, lu au SHA `582f21e1a314c412b6cc856cbfe22736d2419889`
(`jq -r '.repositories[]|select(.name=="ai-work-supervision")|.sha' clones.json`). Les faits d'état
sont établis dans `cahier-des-charges.md` § 1 ; ce document porte la décision de produit. Le
positionnement est la « décision de mission du 2026-10-14 » (`CADRAGE.md:40-61`), hypothèse tant
qu'elle n'est pas devenue ADR signé.

## 1. Vision

Une personne ou une équipe qui confie du travail à des agents de code tient ses missions, ses
décisions et ses preuves dans un journal vérifiable hors de l'outil, sans suivre les conversations
des agents (`docs/architecture/forge/CONTROL-PLANE.md:8`). L'acceptation d'un résultat exige un
commit, une preuve et un résumé ; un critère n'est « vérifié » que si son check a été vu rouge à la
base puis vert au commit soumis (`coordination-v0.md:186-194`).

## 2. Problème

Sans outil, trois choses se perdent : ce qui a été demandé, ce qui bloque, ce qui a été fait
(`cahier-des-charges.md` § 2). Pour Libre-AI, la perte supplémentaire est la preuve du jalon B′ :
dix missions de bout en bout sans fichier de passation hors de l'outil
(`docs/adr/0042-product-vision-and-realization-method.md:78-80`).

## 3. Proposition de valeur

| Bénéficiaire | Valeur | Preuve disponible au SHA lu |
| --- | --- | --- |
| Équipe qui pose les règles (CADRAGE §1.4) | acceptation conditionnée par des checks et des périmètres déclarés ; journal rejouable par un vérificateur sans code partagé | `crates/work-supervision-journal-verifier` ; CA-1, CA-4 |
| Auditeur | rapport `ws report` et journal vérifiable sans accès au daemon | `journal-v0.md:5-8` ; CA-10 (simulation) |
| Propriétaire | décisions signées dans l'outil, passation sans fichier hors outil | non atteint : B′ `pending` (CA-11) |

## 4. Scénarios d'usage

1. **Mission unitaire** : `ws mission new` → `ready` (brief et critères figés) → `run` dans un
   worktree → `result.submit` (commit, preuve, résumé) → `decide` par le propriétaire
   (`daemon-v0.md:45-51`). Observable : événements du journal, `ws report`.
2. **Idée différée** : une session déclarée capture une idée sans interrompre la mission ; le
   propriétaire la promeut ou l'écarte (`coordination-v0.md`).
3. **Demande de décision** : une session ouvre une demande à 2-4 options ; seul `owner` répond
   (`actor.owner_only` pour toute autre identité).
4. **Session hors fabrique** : `ws hook HARNESS` transmet l'état d'une session Claude Code,
   Codex ou Pi, sans texte (`harness-adapters-v0.md:25-32`).
5. **Audit** : rejeu de `ws-journal-verify` et de `ws rebuild --check` sur une copie du journal.

## 5. Métriques associées à une décision

| Métrique | Mesure (commande) | Décision déclenchée | Seuil |
| --- | --- | --- | --- |
| Missions réelles acceptées avec un profil d'exécuteur réel attesté (CA-3, CA-15) | compte d'événements `mission.accepted` hors `"simulation": true` dans le dossier B′ | passage de la fiche `b-prime` de `pending` à atteint (CA-11, première moitié de B′) | 10 consécutives, journal `valid` |
| Attestation `harness-attestation.v1` d'`execution-sandbox` embarquée et stockée pour un run de ce dépôt (le « verdict C0 » n'a pas d'artefact, `cahier-des-charges.md` § 1.5) | `grep -rl libre-ai-harness .clones/ai-work-supervision` (→ 0 au SHA lu) ; attestation vérifiée par un opérateur sans accès au run | levée du refus de profil d'exécuteur réel (CA-3, CA-15) | ≥ 1 fichier et une attestation vérifiée, revu par PR |
| Mission créée avec un exécuteur autre que `fake` dans le journal du propriétaire, par volume de commits du dépôt | `grep '"kind":"mission.created"' JOURNAL \| grep -vc '"executor":"fake"'` et `git rev-list --count 582f21e1..HEAD` | application d'AB-2 (Q1 = A) | 0 mission réelle et ≥ 50 commits (seuil proposé, Q12) |
| Coût machine d'ouverture à 10 000 entrées | test ignoré par défaut (`tests-et-evaluation.md` § 4.3.5) | maintien ou levée de la dérogation I-09 (SQLite) | temps de vérification et de rejeu publiés ; seuil à fixer par Q9 (CA-14 est une mesure sans critère de passage d'ici là) |
| Cycle de déclarations E22 et charge du code gelé dans `bun run check` | `grep -l 'ai-work-supervision/packages/auth-web' .clones/*/package.json \| wc -l` (→ 8) ; contrôles de `package.json:48` (→ 12) | scission du code récupéré (Q3) | cycle de déclarations non nul = scission proposée ; la métrique « lignes de code gelé » n'a pas de seuil et est retirée |
| Vérificateurs tiers qualifiés sur le corpus adverse | résultat du corpus § 4.3.3 sur un second vérificateur | publication du format de journal comme standard (§ 8) | 13/13 codes de refus reproduits |

## 6. Hypothèses à valider

| # | Hypothèse | Expérience | Statut |
| --- | --- | --- | --- |
| H1 | La fabrique tient ses missions sans fichier de passation hors outil | campagne B′ réelle, dossier de preuve | non testée (C0 absent) |
| H2 | Le journal chaîné et la projection SQLite suffisent mono-utilisateur | CA-14 (10 000 entrées) | non mesurée |
| H3 | Un acteur déclaratif suffit tant qu'un seul propriétaire utilise la machine | essai d'usurpation par un second processus du même UID après C0 | non testée, voir Q5 |
| H4 | Le même cœur sert une équipe sous règles (socle) sans réécriture | revue des chemins `tenant` dans `apps/missions` contre le cœur v0 | **contredite par la structure** : la v0 est mono-utilisateur par dérogation et le journal n'a pas de clé tenant (`architecture.md` § 5) ; aucun test multi-tenant sur le cœur v0 |
| H5 | Les organisations réglementées demandent ce journal | usage documenté par une organisation tierce | `[NON VÉRIFIÉ]` ; renfort, non condition (CADRAGE §1.5) |

## 7. Positionnement face aux communs existants

| Commun | Licence | État de maintenance | Recoupement |
| --- | --- | --- | --- |
| `deepseek-ai/deepseek-harness` | MIT | developer preview, aucun audit de sécurité déclaré (`ecosystem/portfolio.v1.yaml:66-89`) | benchmark déclaré ; exclut le confinement ; recoupement des quatre propriétés d'oracle à mesurer (AB-4) |
| `git worktree` | GPL-2.0 `[NON VÉRIFIÉ]` au SHA | maintenu | appelé en processus séparé (ADR-0042 §10) ; non réimplémenté |
| in-toto / DSSE | Apache-2.0 `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | candidat au format de preuve (Q6), via `pi-evidence` |
| Journaux à transparence (Trillian, Sigstore Rekor) | Apache-2.0 `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | recoupement avec l'ancre de tête et le vérificateur hors base ; non évalué |
| Orchestrateurs de tâches d'agents de code (tableaux de missions propres à chaque harness) | variable | `[NON VÉRIFIÉ]` | aucun relevé de licence ni de maintenance fait dans cette session |
| MCP / ACP (protocoles de harness) | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | « integration_possible » à la fiche d'inventaire (`inventaire/ai-work-supervision.json`, `chevauchements_communs`) ; aucune implémentation MCP dans le dépôt |
| OpenTelemetry (traces et journaux d'agents) | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | « a_verifier » ; aucune mention dans le dépôt ; à confronter à I-33 (aucun horodatage par appel hors du harness) |
| Exécution durable (Temporal, Kestra) | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | « a_verifier » ; la reprise de missions (worktree, daemon, journal) en recoupe une partie |

Les lignes `[NON VÉRIFIÉ]` n'ont pas été confirmées : aucun accès réseau n'a été employé hors des
clones épinglés. Une phase ultérieure les rejoue (`gh api repos/ORG/DEPOT --jq .license.spdx_id`).

Tableau « propriété d'oracle × commun » (AB-4) : les quatre propriétés sont P1 journal vérifiable
hors base, P2 reprise après crash prouvée, P3 critères rouges à la base, P4 acceptation à l'octet.
Chaque cellule est `[NON VÉRIFIÉ]` tant que le commun n'a pas été lu : aucune cellule n'est
étayée, et le benchmark déclaré porte sur un autre axe.

| Commun | P1 | P2 | P3 | P4 | Source |
| --- | --- | --- | --- | --- | --- |
| `deepseek-ai/deepseek-harness` | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | axe déclaré = surface de plugins (`ecosystem/portfolio.v1.yaml:66-89`) ; aucun relevé des quatre propriétés |
| MCP / ACP | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | protocoles, pas des orchestrateurs : cellules non applicables à vérifier |
| OpenTelemetry | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | `chevauchements_communs`, « a_verifier » |
| Temporal / Kestra | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | `chevauchements_communs`, « a_verifier » |
| in-toto / DSSE, Trillian / Rekor | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | `[NON VÉRIFIÉ]` | lignes ci-dessus ; candidats au format de preuve (Q6) |

## 8. Types de livrable

Décision recommandée : **méthode + service local** pour le cœur v0 conservé ; **standard** pour le
format de journal et le rapport, à condition d'un second vérificateur qualifié (§ 5, dernière
ligne) ; code récupéré : **sans autorité sur la v0** (`apps/missions/AGENTS.md:1-15`) et conservé
comme référence gelée de la v1 tant que Q3 n'est pas tranchée (`cahier-des-charges.md` § 13,
ligne 7) ; son abandon est l'option C de Q3, pas une décision de ce document.

## 9. Lectures de E3 (non tranchée)

**Lecture A — outil de la fabrique.** ADR-0042 §1 : « relève du produit zéro, la méthode (I-13).
Ce n'est pas une application du catalogue » ; l'ADR 2026-09-29 point 4 le désigne « initial Forge
product home ». Conséquences : pas de ligne P-xx, utilisateur unique (propriétaire), exemption du
palier macOS natif (ADR-0042 §8), jalon B′ comme seule preuve ; la fiche `kind: application` et le
rôle d'inventaire `reserved-application-home` sont à corriger.

**Lecture B — composant du socle avec acteurs externes.** La décision de mission du 2026-10-14
place la « supervision du travail » dans le socle libre + services, premier bénéficiaire :
organisations réglementées autorisant des agents de code. Conséquences : utilisateurs multiples et
acteurs authentifiés (aujourd'hui déclaratifs, RK-2), contrats publiés (§ 2.3 de
`architecture.md`), multi-tenant (phase `admission` `pending`, `project.v1.yaml:42-49`),
levée de la dérogation I-09, services d'hébergement et de certification dans la roadmap, et
contradiction à lever avec ADR-0042 §1 par amendement.

**Ce qui ne change pas selon la lecture** : le cœur v0, ses tests, CA-1 à CA-10, CA-12 à CA-18 et
le refus de tout agent réel avant C0 (CA-13, la licence effective, est identique dans les deux
lectures : EUPL-1.2, I-11). **Ce qui change** : les exigences d'authentification et d'autorisation
(SPECIFICATION-STANDARD section 7), de données avec clé tenant et rétention (section 6) ne
s'appliquent à la v1 qu'en lecture B. La décision est posée en Q1 de `questions-ouvertes.md`.

## 10. Devenir du projet

**Recommandation : conserver**, avec scission du code récupéré gelé (`apps/missions`,
`apps/specifications`, `packages/auth-web`) vers un emplacement qui sort de la `composition` de
`bun run check` et du cycle E22, sans toucher au cœur Rust. Ce déplacement vise du code que
`apps/missions/AGENTS.md:3, 10-12` gèle par la décision propriétaire Y31 : il exige une nouvelle
décision du propriétaire, portée par Q3 (options A, B, C), et n'est pas une recommandation
exécutable sans elle.

Argumentaire : le cœur v0 est le seul exécutant de la méthode sur ses propres missions
(`cahier-des-charges.md` § 1.2 : 46 commits, 5 runs verts, campagne simulée 10/10) ; aucun commun
relevé ne couvre les quatre propriétés d'oracle : **non établi**, car aucun relevé n'a été fait
(§ 7 : cinq candidats en `[NON VÉRIFIÉ]`) ; la fusion avec un autre projet
n'est pas appliquée (liste validée, HANDOFF-phase-0 §7).

Chevauchement avec `organization-data-lifecycle`, ajouté au recoupement de seconde famille du
2026-10-15. Le paquet `@libre-ai/data` est importé par `apps/missions` et `apps/specifications`, dans
11 fichiers. Toute règle locale de rétention ou d'effacement (ENF-2, CA-16, Q8) doit d'abord être
confrontée à `ActiveDeletionRequest`.

Autres chevauchements nommés, sans fusion de specs : `execution-sandbox` (C0, dépendance
bloquante de B′ ; CA-3, CA-15, RK-1 ; attend aussi que ce dépôt embarque `libre-ai-harness`), `pi-evidence` (format
de preuve, Q6), `capability-authorization` (acteur déclaratif et autorisation, Q5 ; la moitié
applicative de la matrice, `apps/missions/src/authz/mission-authorization.ts:1-7`, est rangée ici
dans le code gelé), `signalement` (capture et Dossier ; il porte P15, projets, tickets et support :
ADR-0055 D2, D94 ; ce dépôt est la fabrique et ne porte aucune application du catalogue),
`execution-continuity-evaluator`
(P35, reprise ; attend un adaptateur `RunControlState` / `StartPreflight` et le rejeu de dix
missions, et partage les familles `execution-plan-body`, `orchestrator-event`,
`execution-authorization`), `artifact-verification` (sa lignée de contribution signée est
attribuée à « Work Supervision », fonction que ce cahier n'inscrit pas ; elle recoupe `ws report`),
`ai-model-policy` (contrat d'appel attendu avec la supervision), `database-policy-inspector`
(les 15 fichiers `.sql` de ce dépôt servent de corpus), `personal-knowledge-workspace` (seconde
moitié de B′, boucle documentaire). Le format de chaîne du journal v0 est un second format de
chaîne d'événements à côté de `orchestrator-event.v1..v3` (`previousEventDigest`, `eventDigest`,
`sequence`, `contracts/fixtures/agent-orchestration-v1/event-chain-vectors.v1.json`) : I-03 (un
sujet, une autorité) demande leur comparaison, non faite ici (`architecture.md` § 2.3).
Critère de revue de phase 2 : le regroupement R1/R2 de `inventaire/synthese.md` §7.

Moyens de validation : AB-1 à AB-4 (conditions d'abandon vérifiables) ; CA-11 (dix missions
réelles) pour confirmer ; CA-14 pour la dérogation I-09 ; second vérificateur sur le corpus adverse
pour le passage en standard.

Contre-hypothèse retenue ouverte : si E3 est tranchée en lecture A seule, la ligne `kind:
application` est une erreur de fiche et le projet n'entre pas au catalogue ; si en lecture B, la
décision « conserver » tient mais l'ordre de réalisation d'ADR-0042 §3 est à amender.

## 11. Paquets de travail

Le tableau ci-dessous découpe les critères d'acceptation que `cahier-des-charges.md` § 9 déclare
non satisfaits au SHA lu. Le cahier n'a pas de table d'état : le statut est lu dans le texte de
chaque critère, retenu quand il dit « non atteint », « non atteignable », « aucun test » ou « à
ajouter ». Les critères CA-1 à CA-4, CA-10, CA-12 et CA-14 n'y figurent pas : leur texte ne
déclare aucun manque, ou, pour CA-14, il pose une mesure sans critère de passage. Le critère CA-13
n'y figure pas non plus : il est déclaré non atteint au SHA lu `582f21e1`, mais le commit suivant
de la branche servie (#36) aligne `LICENSE` et `REUSE.toml` sur EUPL-1.2 ; ses observables locaux
(`head -1 LICENSE`, `ls LICENSES`) sont tenus, la résolution GitHub et `reuse lint` ne sont pas
rejoués ici.

Chaque périmètre ne nomme que des chemins suivis du dépôt. Porte : un paquet qui n'ajoute que des
tests est `two-way` (un retrait de test se défait par une PR) ; le paquet qui ajoute un profil
d'exécuteur réel est `one-way`, car il lève le refus de tout agent réel avant C0 et ses effets
hors confinement ne se défont pas, et il ajoute une dépendance et change la fiche publiée. Les
paquets portant CA-15, CA-11 et CA-19 dépendent de C0 et de `execution-sandbox` (§ 10) ; le paquet
de CA-19 précède celui de CA-15 et CA-11.

| WP | CA-n couverts | Périmètre (chemins, globs) | Vérification (commande) | Porte anticipée |
| --- | --- | --- | --- | --- |
| `WP-1` | `CA-5` | `crates/work-supervision-cli/tests/b_prime_simulation.rs` | `cargo test -p work-supervision-cli --locked --test b_prime_simulation` | `two-way` |
| `WP-2` | `CA-6` | `crates/work-supervision-daemon/tests/daemon.rs`, `crates/work-supervision-journal-verifier/tests/cli.rs` | `cargo test -p work-supervision-daemon -p work-supervision-journal-verifier --locked` | `two-way` |
| `WP-3` | `CA-9` | `crates/work-supervision-cli/tests/hook.rs` | `cargo test -p work-supervision-cli --locked --test hook` | `two-way` |
| `WP-4` | `CA-7` | `crates/work-supervision-cockpit/tests/cockpit.rs` | `cargo test -p work-supervision-cockpit --locked` | `two-way` |
| `WP-5` | `CA-8` | `crates/work-supervision-daemon/tests/daemon.rs` | `cargo test -p work-supervision-daemon --locked --test daemon` | `two-way` |
| `WP-6` | `CA-16` | `crates/work-supervision-store/tests/projection.rs`, `crates/work-supervision-journal-verifier/tests/verify.rs` | `cargo test -p work-supervision-store -p work-supervision-journal-verifier --locked` | `two-way` |
| `WP-7` | `CA-17` | `crates/work-supervision-daemon/tests/anchor.rs` | `cargo test -p work-supervision-daemon --locked --test anchor` | `two-way` |
| `WP-8` | `CA-18` | `crates/work-supervision-cli/tests/cli.rs` | `cargo test -p work-supervision-cli --locked --test cli` | `two-way` |
| `WP-9` | `CA-19` | `crates/work-supervision-domain/tests/transitions.rs` | `cargo test -p work-supervision-domain --locked --test transitions` | `two-way` |
| `WP-10` | `CA-15`, `CA-11` | `Cargo.toml`, `Cargo.lock`, `deny.toml`, `crates/work-supervision-domain/src/model.rs`, `project.v1.yaml` | `bun run check:native` | `one-way` |
