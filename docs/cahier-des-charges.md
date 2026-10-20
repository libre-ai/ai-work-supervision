# ai-work-supervision — cahier des charges

Maturité : prototype

Provenance : ce document reprend la spécification de mission
`specs/ai-work-supervision/cahier-des-charges.md` de l'analyse du 2026-10-14. Seuls changements :
des réécritures de forme exigées par la norme des paquets de travail de
`libre-ai/project-governance` (`docs/specifications/WORK-PACKAGES.md`) : chaque paramètre écrit
entre chevrons y est écrit en capitales (`HARNESS`, `ID`, `CODE`, `L`, `JOURNAL`, `ORG/DEPOT`).
Le texte des critères CA-1 à CA-19 est inchangé à ces réécritures près. `prd.md` est à côté de ce
fichier ; `architecture.md`, `tests-et-evaluation.md`, `questions-ouvertes.md`, `CADRAGE.md`,
`HANDOFF-phase-0.md`, `clones.json`, `inventaire/` et `specs/` désignent l'analyse de mission et ne
sont pas dans ce dépôt.

Portée du niveau : « prototype » qualifie le **projet observé** au SHA lu (cœur v0 exécuté
seulement au faux agent, aucun usage réel, B′ `pending`, `project.v1.yaml:34-41`), pas ce
document, qui est une proposition de la mission. La fiche porte `maturity: idea` (`:27`) : échelle
de la fiche et échelle du brief (règle 4) diffèrent, et l'écart est consigné sans être arbitré.

Projet : `libre-ai/ai-work-supervision` (« Libre AI Work Supervision », `docs/decisions/LEXICON.md:45`), lu au SHA
`582f21e1a314c412b6cc856cbfe22736d2419889`
(`jq -r '.repositories[]|select(.name=="ai-work-supervision")|.sha' clones.json`). Les chemins
sans préfixe sont relatifs à `.clones/ai-work-supervision/` ; ceux de la doctrine sont relatifs à
`.clones/project-governance/` (SHA `c600e5ddea2a53d0475b79708b9ecfc54e5651ce`), où `INVARIANTS.md`,
`LEXICON.md` et `DECISION-REGISTER.md` vivent sous `docs/decisions/` ; ceux des contrats
à `.clones/schemas-and-contracts/` (SHA `216ffb9207f1afc831d50c6c97b7020da9bb7ae1`). Aucun code
exécuté (CADRAGE §4.15, `CADRAGE.md:150`).

## 1. État des lieux

### 1.1 Ce que le dépôt contient

Trois générations de code coexistent dans un même arbre (fiche d'inventaire,
`inventaire/ai-work-supervision.json`, `contradictions_doc_code[2]`) :

1. **Le cœur natif v0** : 10 crates Rust (`Cargo.toml:6-17`), édition 2024, `rust-version` 1.97,
   dépendances épinglées à la version exacte (`Cargo.toml:27-39`), lints refusant `unwrap`,
   `expect`, `panic`, `indexing_slicing` et `unsafe` (`Cargo.toml:41-52`). Rôles :
   `domain` (machine d'états pure, `docs/work-supervision/mission-v0.md:3-5`), `journal` (writer
   chaîné par empreintes) et `journal-verifier` (vérificateur réimplémenté sans code partagé,
   `journal-v0.md:10-18`), `store` (projection SQLite reconstruite depuis le journal,
   `events-v0.md:82-92`), `worktree` (git du système en processus séparé, `worktree-v0.md:7-13`),
   `pty` (terminaux à environnement vidé, `pty-v0.md:9-28`), `daemon` (`wsd`, seul écrivain,
   socket Unix 0600 avec contrôle d'UID du pair, `daemon-v0.md:21-38`), `cli` (`ws`, TUI
   `ws attach`, pont `ws hook`, `ws report`), `cockpit` (HTML rendu côté serveur sur `127.0.0.1`,
   lecture et décision seulement, `cockpit-v0.md:7-31`), `fake-agent` (seul exécuteur,
   `pty-v0.md:3-5`). Spécifications v0 dans `docs/work-supervision/` (11 fichiers).
2. **Le code récupéré, gelé** : `apps/missions` (domaine multi-tenant V6, référence sans autorité
   sur la v0, `apps/missions/AGENTS.md:1-15`), `apps/specifications`, `packages/auth-web`
   (OIDC + PKCE, session opaque, CSRF ; deux portes humaines non acceptées,
   `packages/auth-web/G2-I01-QUALIFICATION.md:5-7`). Provenance par fichier dans
   `docs/code-recovery-provenance.json` (schéma `code-recovery.v1`).
3. **Les adaptateurs** : `adapters/pi/ws-bridge.ts` (extension Pi du pont déclaratif,
   `harness-adapters-v0.md:102-110`) et l'outillage de qualité `tools/quality/` (cargo-deny
   épinglé, chemins machine).

### 1.2 Activité et preuve

- 46 commits en 90 jours, 3 contributeurs, dernière PR fusionnée #35 le 2026-10-14 (fiche,
  `activite.source`). Les PR #8 à #35 construisent la v0 par tranches T2 à T10
  (`git -C .clones/ai-work-supervision log --oneline -46`).
- CI `Code validation` : 5 derniers runs `success` au 2026-10-14 (fiche, `ci.derniers_runs`),
  via `validate-composition.yml@0db2e10e` (`.github/workflows/code-validation.yml:13`).
- Fiche `project.v1.yaml` : `kind: application`, `layer: couche-2` (`:4-5`), `maturity: idea`
  (`:27`), phase `v0` avec critère `b-prime` **pending** (`:34-41`), phase `admission` (v1
  multi-tenant) **pending** (`:42-49`), `benchmark.none` (`:50-51`). La situation courante
  (`:15`) décrit encore le snapshot récupéré `d0ce4046` alors que 46 commits et 5 runs verts ont
  suivi : fiche partiellement fiable (fiche, `documentation.fiabilite`).
- Campagne B′ **simulée** au faux agent : 10/10 missions acceptées, journal 157 entrées, ancre
  `matched`, reconstruction égale, 0 worktree, 0 fichier parasite ; ne compte pas pour B′
  (`docs/work-supervision/b-prime-rehearsal.md:1-7, 33`).

### 1.3 Place dans la doctrine

- Inventaire : `role: reserved-application-home`, `layer: couche-2`, « human surface of agent
  missions, specifications and web authentication », « initial Forge product home (ADR
  2026-09-29 §4) » (`ecosystem/repositories.v1.yaml:55-63`).
- ADR 2026-09-29 : « Forge is its integrated product factory » (point 1, l. 14) ; « Use
  ai-work-supervision as the initial Forge product home » (point 4, l. 17).
- ADR-0042 (`proposed`, fusionné) : la fabrique « relève du produit zéro, la méthode (I-13). Ce
  n'est pas une application du catalogue » ; ses deux composants sont Work Supervision ici et le
  confinement, outillage privé puis `execution-sandbox` après C0
  (`docs/adr/0042-product-vision-and-realization-method.md:52-56`) ; ordre de réalisation : cœur
  Rust, CLI/TUI, cockpit web en lecture et décision, « aucun agent réel … avant que C0 soit vert »
  (`:89-93`) ; dérogation nommée à I-09 pour la seule v0 mono-utilisateur, SQLite, journal chaîné
  vérifiable hors base, schéma portable vers PostgreSQL, configuration privée hors dépôt
  (`:138-145`) ; la fabrique n'est pas concernée par le premier palier macOS natif (`:221-222`) ;
  `git` du système appelé en processus séparé n'est pas une adoption GPL (`:273` sqq.).
- Jalon B′ : « dix missions consécutives de bout en bout dans Work Supervision, sans fichier de
  passation hors de l'outil, et une première utilisation réelle de la boucle documentaire »
  (`0042…:78-80`).
- Design de la fabrique dans `docs/architecture/forge/` : frontière « Mission truth, decisions and
  integration; no ownership of application data or Git truth »
  (`REPOSITORY-ARCHITECTURE.md:18`) ; objectif du plan de contrôle « supervise multiple missions
  without following agent conversations » (`CONTROL-PLANE.md:8`), « No UI owns state » (`:29`).
- Benchmark de parité déclaré : `deepseek-ai/deepseek-harness` (MIT, developer preview, aucun audit
  de sécurité), axe « étendue et composabilité de la surface de plugins », explicitement **pas** le
  confinement (`ecosystem/portfolio.v1.yaml:66-89`) ; `delta.dev` en référence exclue, licence non
  publiée (`:90` sqq.).

### 1.4 Contradictions constatées (instruites dans `questions-ouvertes.md`, jamais réécrites)

| # | Constat | Sources |
| --- | --- | --- |
| E3 | Outil de la fabrique hors catalogue (ADR-0042 §1) **ou** application du catalogue (`project.v1.yaml:4-5`, `README.md:4` « This proposed product », inventaire `reserved-application-home`) **ou** composant du socle « libre + services » (décision de mission du 2026-10-14, `CADRAGE.md:40-61`) | `inventaire/synthese.md:185` |
| E10 | Licence : `LICENSE:1-3` texte MIT au nom d'une personne, `REUSE.toml:3-7` annote `**` en MIT (`aggregate`), GitHub résout MIT ; `REUSE.toml:9-14` ajoute une annotation EUPL-1.2 `closest` sous cette annotation `aggregate` ; `LICENSES/` contient `EUPL-1.2.txt` et `CC-BY-4.0.txt`, pas de `MIT.txt` ; `package.json:4` et `Cargo.toml:23` disent EUPL-1.2 ; `LICENSING.md:25` impose EUPL-1.2 au logiciel de première partie ; I-11 (`INVARIANTS.md:17`). La spec part de EUPL-1.2 comme licence voulue et consigne l'écart (HANDOFF-phase-0 §7, `HANDOFF-phase-0.md:71`) | `synthese.md:222` |
| E11 | « Forge » désigne la fabrique de produits (ADR 2026-09-29, home ici) et, dans le brief de mission, le système de gates et de doctrine (`project-governance`) ; le LEXICON n'a pas d'entrée « Forge » et laisse « harness » non tranché (`LEXICON.md:151-155`) | `synthese.md:223` |
| E22 | `packages/auth-web` est déclaré par 8 `package.json` hors de ce dépôt (`grep -l 'ai-work-supervision/packages/auth-web' .clones/*/package.json | wc -l` → 8, dont `ai-model-policy`, `application-development-toolkit` et `travel-itinerary-planner/package.json:55`), et les deux premiers sont eux-mêmes consommés par ce dépôt : une application de couche 2 en amont de briques de couche 4 | `synthese.md:100-113, 234` ; `package.json:10-23` |
| — | `check:specifications --apps missions agent-board` et `docs/apps/agent-board.md` nomment une application absente de l'arbre (`ls apps` → `missions`, `specifications`) | `package.json:49` |
| — | Le README présente un « proposed product » destiné aux « people coordinating work with agents » (`README.md:4`) ; ADR-0042 §1 le place hors catalogue | fiche, `documentation.justification` |

### 1.5 Ce que « verdict C0 vert » désigne au SHA lu

Aucun artefact ne matérialise ce verdict. `grep -rln 'C0\|gondolin' .clones/execution-sandbox`
(SHA `d1ffdd2bb2f2c8398861c041cf622dfb78a42e64`) ne renvoie que `bun.lock` et
`LICENSES/CC-BY-4.0.txt` ; la spec d'`execution-sandbox` écrit que l'état de C0 « n'est pas
observable dans un dépôt public » (`specs/execution-sandbox/tests-et-evaluation.md:348`). Le garde
du domaine ne lit pas de verdict : `ExecutorProfile` n'a qu'une variante, `Fake`
(`crates/work-supervision-domain/src/model.rs:86-89`), et le lever est un changement de code revu
(`harness-adapters-v0.md:129-131`).

Cette spec n'emploie donc plus « verdict C0 vert publié » comme objet. Les critères CA-3, CA-11 et
AB-1 se lisent sur trois observables, par ordre de force croissante : (1) une revue de code
ajoutant une variante à `ExecutorProfile` cite le chemin d'une attestation conforme à
`harness-attestation.v1` (`.clones/schemas-and-contracts/contracts/schemas/harness-attestation.v1.schema.json`,
verrouillé, `specs/execution-sandbox/architecture.md:83`) ; (2) le dépôt embarque la bibliothèque
`libre-ai-harness` d'`execution-sandbox` (`grep -rl libre-ai-harness .clones/ai-work-supervision` ≥ 1, CA-15) ;
(3) un run réel de ce dépôt produit une attestation stockée et vérifiée par un
opérateur sans accès au run (`specs/execution-sandbox/cahier-des-charges.md:232`, CA-16 de ce
projet-là). Qui signe « C0 vert », et sur quel artefact, est la question Q11.

## 2. Problème résolu

Une personne qui confie du travail à des agents de code perd trois choses sans outil : ce qu'elle a
demandé (le brief dérive avec la conversation), ce qui bloque (une question de l'agent attend dans
un terminal), et ce qui a été réellement fait (l'agent rapporte, personne ne vérifie). Le dépôt
pose ces trois pertes comme ses usages (`project.v1.yaml:16-19`) et y répond par un **journal
d'autorité** chaîné par empreintes, une **machine d'états** fermée où l'acceptation exige commit,
preuve et résumé (`mission-v0.md:35`), des **critères vérifiés par des checks** vus rouges à la
base puis verts au commit soumis (`coordination-v0.md:186-194`), et un **pont** par lequel les
sessions d'agents lancées hors de la fabrique déclarent leur état sans que leur texte entre dans
l'outil (`harness-adapters-v0.md:3-8, 25-32`).

Le problème de second niveau, propre à Libre-AI : la méthode est le produit zéro (I-13,
`INVARIANTS.md:19`) et chaque jalon se prouve par l'usage du propriétaire (I-19, `:25` ; CADRAGE
§1.5, `CADRAGE.md:73`). Sans un outil qui tienne les missions, les passations se font par fichiers
hors outil, ce que B′ interdit explicitement (`0042…:78-80`).

## 3. Utilisateurs cibles

| Utilisateur | Rôle au SHA lu | Rôle visé par la décision de mission |
| --- | --- | --- |
| **Propriétaire** (acteur `owner`) | seul décideur : `ready`, `run`, `accept`, `reject`, `abandon`, `cancel`, déclaration des dépendances, périmètres, checks, phases ; répond aux demandes de décision (`coordination-v0.md:25-35`) | inchangé pour la v0 ; devient « l'équipe qui pose les règles » (CADRAGE §1.4, `CADRAGE.md:66-72`) si le projet entre au socle |
| **Sessions d'agents** (acteur `session:ID`) | déclarent leur état par le pont, capturent des idées, ouvrent des demandes de décision, soumettent des artefacts de phase ; refusées sur toute décision (`actor.owner_only`) | exécuteurs réels lancés par la fabrique après C0 (`harness-adapters-v0.md:114-134`) |
| Développeur sous règles | absent de la v0 (mono-utilisateur) | utilisateur secondaire (CADRAGE §1.4) : lit `ws report`, ne décide pas |
| Auditeur | lit `ws report --markdown` et le journal vérifié hors base (`journal-v0.md:5-8`) | utilisateur secondaire : rejoue `ws-journal-verify` et `ws rebuild --check` sans accès au daemon |

L'utilisateur primaire du socle est l'équipe qui pose les règles (CADRAGE §1.4) ; au SHA lu, cette
équipe est une personne, le propriétaire, et la configuration déclarative est `config.toml`
(`crates/work-supervision-daemon/src/config.rs:9-24`). Pour P15 et P35, que le catalogue associe à
ce dépôt sans qu'il les nomme (`inventaire/catalogue-p-xx.json`, `depot_porteur: null`,
`grep -rlE '\bP15\b|\bP35\b' .clones/ai-work-supervision` → 0), l'utilisateur est le propriétaire.

## 4. Périmètre

### 4.1 Dans le périmètre (v0, au SHA lu)

- Missions : brief et critères figés à `ready`, worktree par mission, un run par terminal,
  résultat = commit + fichier de preuve + résumé, décision du propriétaire (`mission-v0.md:9-25`).
- Journal d'autorité et projection SQLite reconstructible ; ancre de tête hors racine.
- Coordination : idées différées, demandes de décision à 2-4 options, dépendances, périmètres,
  checks, sessions déclarées, découpe d'outline, frontière parallèle « fusionné, pas clos »
  (`coordination-v0.md`).
- Phases et artefacts approuvés à l'octet (`phases-v0.md`).
- Interfaces : `ws` (CLI), `ws attach` (TUI), `ws-cockpit` (web local), `ws hook` (pont), rapport
  JSON et Markdown.
- Pont déclaratif vers Claude Code, Codex, Pi ; contrat d'adaptateur d'exécuteur **spécifié, non
  activé** (`harness-adapters-v0.md:9-15`).

### 4.2 Hors périmètre

- Lancer un agent réel avant C0 : refusé par construction (`agent.real_forbidden_until_c0`).
- Confinement : qualifié dans `execution-sandbox` (ADR-0042 §1) ; ce projet consomme le verdict.
- Évaluation de continuité d'effet et graphe autorisé : `execution-continuity-evaluator`
  (`docs/architecture/forge/REPOSITORY-ARCHITECTURE.md:20`).
- Pousser sur la forge : « Work Supervision never pushes » (`coordination-v0.md:305-306`).
- Multi-tenant, rôles, approbation tierce : dérogation I-09 pour la v0 (`0042…:138-145`) ; la
  phase `admission` v1 est déclarée mais sans critère atteint (`project.v1.yaml:42-49`).
- Toute application du catalogue P01-P40 : la fabrique est hors catalogue (ADR-0042 §1) ; P15 et
  P35 lui sont rattachés par le catalogue de phase 0 seulement (voir `prd.md` § Devenir).
- Lecture du texte des agents par une règle du daemon : « no rule of the daemon reads its
  content » (`phases-v0.md:116-118`).

## 5. Exigences fonctionnelles

- EF-1 Toute mutation est une commande du domaine pur, refusée fermée par un code stable, avec
  révision optimiste (`mission.revision_stale`), et produit exactement un événement journalisé
  avant d'être projetée (`mission-v0.md:27-37, 46-52`).
- EF-2 Une acceptation exige un commit, un digest de preuve et un résumé non vide ; la tête du
  worktree doit être le commit soumis ; chaque critère porteur d'un check doit être vérifié au
  commit soumis **et** rouge à la base (`coordination-v0.md:124-143`).
- EF-3 Une mission bloquée dit ce qu'elle attend : liste de codes de blocage dans `ws mission
  show`, le cockpit et `ws report` (`coordination-v0.md:148-149`).
- EF-4 Les phases déclarées (1 à 7, ordre canonique) ferment chacune sur un artefact approuvé par
  digest ; une soumission ultérieure rend caduques les approbations des phases suivantes
  (`phases-v0.md:25-36, 101-111`).
- EF-5 Une session d'agent externe se déclare par un payload de hook traduit en `working`,
  `waiting-input`, `blocked`, fin ; la note est un mot machine, jamais le message de l'agent
  (`harness-adapters-v0.md:34-42` ; `crates/work-supervision-cli/src/hook.rs:57-62`).
- EF-6 Le rapport d'une mission se lit seul : demandé, fait, preuves, écarts (codes), décisions,
  blocages, chronologie ; forme JSON = contrat, Markdown = rendu, textes échappés
  (`coordination-v0.md:345-359`).
- EF-7 Les worktrees sont créés et libérés par intention journalisée avant l'effet git et
  confirmation après ; la réconciliation au démarrage et `ws worktree gc` ne laissent aucun
  orphelin (`worktree-v0.md:19-52`).
- EF-8 Le propriétaire répond à une demande de décision parmi 2 à 4 options portant conséquence et
  réversibilité, affichées côte à côte (`coordination-v0.md:74-78`) — c'est I-24
  (`INVARIANTS.md:30`) porté dans l'outil.

## 6. Exigences non fonctionnelles

Cadres réglementaires de CADRAGE §1.3 (`CADRAGE.md:62-65`) : chaque exigence marque ceux qu'elle
sert. Les qualifications juridiques ci-dessous sont des hypothèses de la mission, pas un avis ;
elles sont marquées comme telles.

- ENF-1 **Intégrité contre l'altération non coordonnée** : journal chaîné SHA-256 sur forme
  canonique RFC 8785, vérificateur indépendant, ancre hors racine **optionnelle** (`[anchor]` absent
  de `config.toml` : `ws journal verify` rend `not-configured`, `daemon-v0.md:101`). La chaîne est
  une empreinte sans clé ni signature (`journal-v0.md:29-37`) et l'ancre est un fichier 0600 du
  même compte, écrasé après chaque requête (`daemon-v0.md:87-95`) : un processus du même UID peut
  réécrire journal et ancre de façon cohérente. **La non-répudiation n'est pas fournie.** Cadres : NIS2 (journalisation et
  détection, hypothèse), DORA (traçabilité des TIC pour une entité financière qui adopterait
  l'outil, hypothèse), AI Act art. 12 (tenue de journaux par le déployeur d'un système à haut
  risque — s'applique seulement si un agent supervisé relève d'un tel système, hypothèse
  `[NON VÉRIFIÉ]`). Validation : § 3 de `tests-et-evaluation.md`.
- ENF-2 **Minimisation** : aucun texte libre dans le journal ; titres, briefs, critères, notes,
  résumés, raisons en blobs adressés par contenu, effaçables sans casser la **chaîne**
  (`events-v0.md:13-17`) : `ws-journal-verify` ne lit pas les blobs. L'effacement casse en
  revanche la **reconstruction** : `ws rebuild` refuse dès qu'un blob nommé par le journal manque
  (`StoreError::BlobMissing`, `crates/work-supervision-store/src/projection.rs:121-126, 275-281`),
  donc `ws rebuild --check` n'a plus de verdict à ce `seq` et l'oracle de CA-2 tombe. Aucune
  commande `ws` n'efface un blob au SHA lu : la propriété « effaçable » n'est exercée par aucun
  test (CA-16). Cadre : RGPD (minimisation, droit à l'effacement sur les blobs ; le
  digest d'un texte effacé reste dans la chaîne — question Q8 de `questions-ouvertes.md`, qui porte
  aussi le traitement du blob manquant).
- ENF-3 **Zéro donnée personnelle en journal d'exploitation** : stderr du daemon = compteurs
  seulement (`daemon-v0.md:32-33`) ; sortie du vérificateur = codes, numéros de ligne, comptes
  (`journal-v0.md:93`) ; identifiant de session d'un harness jamais transmis, seulement son SHA-256
  (`hook.rs:5-8`). Cadre : RGPD ; standard de spécification « Logs contain identifiers, rule IDs,
  timings and outcomes—not content » (`docs/specifications/SPECIFICATION-STANDARD.md`, § Data and
  privacy baseline).
- ENF-4 **Récupérabilité** : reprise verte après `SIGKILL` réel à 14 points du daemon et 5 de la
  coordination, **en build debug seulement** (`crates/work-supervision-daemon/src/fault.rs:1-6` :
  « Release builds compile [`hit`] to nothing ») ; après une erreur injectée puis abandon du
  verrou (pas un `SIGKILL`) à 6 points de worktree (`crates/work-supervision-worktree/src/lib.rs:85-103`,
  `tests/lifecycle.rs:379`). Le build release n'est pas couvert. `ws rebuild` depuis journal et
  blobs **présents** (voir ENF-2). Cadres : NIS2 et DORA (continuité, hypothèse).
- ENF-5 **Supervision humaine** : aucun avancement automatique de phase ; « every phase closes on
  the owner's approval » (`phases-v0.md:120-123`) ; un run ne passe jamais un checkpoint seul
  (`harness-adapters-v0.md:142-145`). Cadre : AI Act art. 14 (surveillance humaine, hypothèse).
- ENF-6 **Surface réseau nulle** : socket Unix 0600 avec UID du pair, aucun port réseau
  (`daemon-v0.md:35-37`) ; cockpit sur `127.0.0.1` seulement, refus de `Host` étranger
  (`cockpit-v0.md:9, 37`). Cadres : NIS2 (réduction de surface) ; RGS / SecNumCloud ne
  s'appliquent pas à un outil local v0 et s'appliqueraient à un service hébergé (hors périmètre).
- ENF-7 **Reproductibilité de la chaîne d'outils** : versions exactes (`Cargo.toml:27-39`),
  toolchain `1.97.0` (`rust-toolchain.toml`), cargo-deny par archive hachée
  (`tools/quality/check-dependency-policy.ts:37-40`), garde d'âge de publication de 3 jours
  (`.github/dependabot.yml`, commentaire « Release-age guard »). Cadres : NIS2 et DORA (chaîne
  d'approvisionnement, hypothèse).
- ENF-8 **Coût machine borné** : budgets par run (durée, octets) avec `SIGTERM` puis `SIGKILL`
  sur le groupe (`pty-v0.md:23-26`) ; au plus 32 dépendances et 32 préfixes de périmètre par
  mission (`coordination-v0.md:96-105`) ; coût des gardes proportionnel à missions × porteurs de
  worktree, « à revisiter avant la phase multi-tenant » (`:163-167`). Non mesuré au SHA lu.

## 7. Interfaces

| Interface | Entrée | Sortie | Format |
| --- | --- | --- | --- |
| Journal | événements du catalogue | fichier UTF-8, une entrée JCS par ligne, six clés `at`, `digest`, `event`, `prev`, `schema`, `seq` | `libre-ai.work-supervision.journal.v0` (`journal-v0.md:29-49`) — **local au dépôt, non publié dans `schemas-and-contracts`** (`:7-8`) |
| Protocole `wsd` | un objet JSON par ligne, ≤ 4 MiB, `op` + champs | `{"data": …, "meta": {"op": …}}` ou `{"error": {"code": …}}` | `daemon-v0.md:40-41` |
| `config.toml` | sous-ensemble TOML strict : `[repositories]`, `[executor]`, `[runs]`, `[anchor]` | refus `config.invalid` avec numéro de ligne | `config.rs:1-24` |
| Ancre | tête du journal ; **optionnelle** : active seulement si `[anchor] path` est déclaré dans `config.toml`, sinon `ws journal verify` rend `not-configured` | une ligne JSON `libre-ai.work-supervision.anchor.v0`, 0600 | `daemon-v0.md:89-93, 101` |
| `ws hook HARNESS` | payload JSON du harness sur stdin ou dernier argument | rien sur stdout, exit 0 toujours | `harness-adapters-v0.md:25-32` |
| Extension Pi | `pi.on(…)` → `{"event", "session_id", "cwd"}` | appel de `ws hook pi` borné à 5 s | `harness-adapters-v0.md:92-110` |
| Plan de découpe d'outline | JSON strict `{ "missions": [{key, title, brief, demonstrates, criteria?, after?, budgets?}] }` | 1 à 32 missions `draft` | `coordination-v0.md:232-255` |
| Rapport | identifiant de mission | JSON (contrat) ou Markdown (rendu) | `coordination-v0.md:345-359` |
| Cockpit | HTTP sur `127.0.0.1`, cookie de session `ws_session=…; Path=/; HttpOnly; SameSite=Strict` (sans `Secure` ni préfixe `__Host-`, `crates/work-supervision-cockpit/src/lib.rs:601`), jeton CSRF | HTML sans script, CSP `default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'` (`lib.rs:917-918`) | `cockpit-v0.md:35-47` ; écarts : § 8.4 |
| Fichier de preuve | fichier remis à `ws result submit`, stocké dans `evidence/` | digest dans l'événement | format **non spécifié** : `daemon-v0.md:45-51` dit seulement « the evidence file is stored by `ws` in `evidence/` » (absence, pas une phrase du document) — question Q6 |
| Migrations | `migrations/sqlite/000{1..6}` et `migrations/postgres/000{1..6}` | tables de projection | `schema.v0.json` commun (`events-v0.md:77-81`) |

## 8. Contraintes de sécurité

### 8.1 Menaces retenues et réponses au SHA lu

| Menace | Réponse | Source |
| --- | --- | --- |
| Altération non coordonnée du journal (troncature, édition, réordonnancement) par un processus sans droit d'écriture sur l'ancre | chaîne d'empreintes + ancre hors racine si `[anchor]` est déclaré ; refus au démarrage. **Non couvert** : un processus du même UID qui réécrit journal et ancre de façon cohérente (aucune clé, aucune signature ; même limite que l'acteur déclaratif, ligne « se déclarant `owner` ») ; non couvert non plus : ancre non configurée | `daemon-v0.md:87-102` ; `coordination-v0.md:16-23` ; test CA-17 |
| Processus d'un autre utilisateur sur le socket | `getpeereid` / `SO_PEERCRED`, `socket.peer_refused` | `crates/work-supervision-daemon/src/peer.rs:30-46` |
| Navigateur atteignant un terminal d'agent | aucune route de log, de PTY ni d'entrée ; test au marqueur | `cockpit-v0.md:29-31` |
| DNS rebinding, CSRF, requête cross-site sur le cockpit | `Host` strict, `Origin` propre, `Sec-Fetch-Site`, cookie `HttpOnly` `SameSite=Strict`, jeton CSRF, comparaison en temps constant | `cockpit-v0.md:35-43` |
| Agent évadant son worktree | **aucun contrôle d'évasion au SHA lu** : `pty.cwd_invalid` refuse un répertoire de travail manquant, l'environnement est vidé et le groupe de processus tué en fin de run, mais rien n'empêche une lecture ou une écriture ailleurs sur le disque ni l'accès au socket ; le confinement est C0, hors dépôt (même limite que les lignes « Processus du même UID » et « Agent réel » ci-dessous) | `pty-v0.md:9-28` ; `architecture.md` § 7.1 |
| Hooks git ou configuration globale détournés | `core.hooksPath=/dev/null`, `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`, argv fixe sans shell | `worktree-v0.md:7-13` |
| Dépendance sous licence non admise ou source inconnue | cargo-deny `bans`, `licenses`, `sources` depuis le check requis ; `allow-org github = ["libre-ai"]` | `deny.toml` ; `check-dependency-policy.ts:40` |
| **Processus du même UID se déclarant `owner`** | **aucune** : « the actor is declarative, not authenticated » ; la frontière attendue est C0 | `coordination-v0.md:16-23` — question Q5 |
| **Agent réel lançant des effets hors confinement** | refus de tout profil autre que `fake` jusqu'à C0 | `mission-v0.md:39-44` |

### 8.2 Anti-injection (I-32, `INVARIANTS.md:38`)

- Aucune règle du daemon ne lit le contenu d'un artefact, d'un commentaire ou d'une raison ; ces
  textes sont affichés échappés, jamais exécutés (`phases-v0.md:116-118`).
- La note d'un hook est réduite à un mot machine ; le message de l'agent n'est jamais enregistré
  (`hook.rs:57-62`).
- Le plan de découpe est la demande du propriétaire, « never inferred from the outline's text,
  which no rule reads » (`coordination-v0.md:251-253`).
- `ws hook` n'écrit rien sur stdout parce que Claude Code ajoute la sortie d'un hook au contexte
  du modèle sur certains événements (`harness-adapters-v0.md:29-31`) : la fabrique ne renvoie
  aucune chaîne vers un modèle.
- Limite : quand un exécuteur réel existera, le brief figé et les critères entreront dans le
  contexte d'un modèle à outils ; I-32 exige alors que tout contenu non fiable lu par ce modèle
  circule en référence opaque. Rien au SHA lu ne l'implémente ni ne le contredit : hors périmètre
  v0, à spécifier avec le profil d'exécuteur.

### 8.3 Pont de harness et I-33

Le pont traduit `PreToolUse` et `PostToolUse` en `working`
(`crates/work-supervision-cli/src/hook.rs:74`, `harness-adapters-v0.md:36`) et chaque rapport est
un `session.reported` journalisé dont l'entrée porte `at` (`coordination-v0.md:325-335`,
`journal-v0.md:35`). La lecture d'I-33 au regard de ce flux, et l'écart relevé, sont en
`architecture.md` § 6 ; la décision est la question Q13 ; le test est CA-18.

### 8.4 Écarts du cockpit à I-06 et I-37

Détail et sources dans `architecture.md` § 11. En bref : CSP sans `script-src 'self'` ni
`object-src 'none'` (le cockpit n'a aucun script), cookie sans `Secure` ni `__Host-` (boucle
locale en HTTP), cockpit web servi par Rust au regard d'I-06 (décision Y30, `cockpit-v0.md:3-5`,
absente de `DECISION-REGISTER.md`). Aucun de ces écarts n'est arbitré par un ADR `accepted`.

### 8.5 Zéro donnée personnelle en journal

Tenu par ENF-2 et ENF-3 pour les événements ; CA-5 en porte le test à écrire. Écart résiduel : la projection SQLite contient les textes en clair
(`migrations/sqlite/0001_projection.sql`, colonnes `title`, `brief`, `summary`, `text`) à côté de
leurs digests ; c'est une projection reconstructible, sous la racine 0700, pas un journal
d'exploitation. Le code récupéré `auth-web` tient sa propre redaction
(`packages/auth-web/evidence/identity-threat-model.md:39`).

## 9. Critères d'acceptation

- CA-1 Le vérificateur indépendant rend `valid` sur le journal de toute campagne de test, et `invalid: CODE at line L` sur chacun des treize codes de refus (douze vérifications ordonnées, `journal-v0.md:56-69`). Validation : tests `crates/work-supervision-journal-verifier/tests/verify.rs` et `tests/cli.rs` verts dans `cargo test --workspace --locked` ; `awk -F'|' '/^\| [0-9]+ \|/{print $4}' docs/work-supervision/journal-v0.md \| grep -oE '`[a-z-]+`' \| sort -u \| wc -l` → 13 ; corpus de treize fichiers adverses, un par code, de `tests-et-evaluation.md` § 4.3.3.
- CA-2 Après un `SIGKILL` réel en build debug à chacun des 14 points du daemon et des 5 points de coordination, puis après une erreur injectée suivie de l'abandon du verrou à chacun des 6 points de worktree, le redémarrage laisse journal valide, projection égale à sa reconstruction, 0 worktree orphelin, 0 branche `ws/*` orpheline ; le build release n'est pas couvert et les 25 ne sont pas 25 points distincts (5 points du daemon passent par un point de worktree, `fault.rs:45-60`). Validation : `recovery_is_green_after_a_kill_9_at_every_injection_point` (`crates/work-supervision-daemon/tests/daemon.rs:376`), `crates/work-supervision-daemon/tests/coordination.rs`, `crates/work-supervision-worktree/tests/lifecycle.rs` ; `cargo test --workspace --locked` sans `--release` (`grep -n 'release' .github/workflows/*.yml package.json` → 0 ligne).
- CA-3 Aucune configuration ne fait démarrer `wsd` avec un profil d'exécuteur autre que `fake` : la variante unique `Fake` de `ExecutorProfile` n'est étendue que par un changement de code revu qui cite l'attestation `harness-attestation.v1` exigée par § 1.5 (aucun artefact « verdict C0 » n'existe au SHA lu). Validation : `a_real_agent_profile_keeps_the_daemon_from_starting` ; test `the_c0_guard_admits_only_the_fake_executor` (`crates/work-supervision-domain/tests/transitions.rs:441-465`, refuse `claude`, `codex`, `pi`, `/usr/local/bin/claude`, `Fake`, la chaîne vide et `fake `) ; `grep -c 'Fake' crates/work-supervision-domain/src/model.rs` ≥ 1 et `ExecutorProfile::ALL` de longueur 1.
- CA-4 Une acceptation est refusée tant qu'un critère porteur d'un check n'est pas vérifié au commit soumis **et** rouge à la base, ou que la tête du worktree a bougé. Validation : tests du daemon sur `criteria.unverified`, `criteria.not_red_at_base`, `worktree.head_moved` ; preuve rouge en substituant le check par un programme qui sort 0 à la base.
- CA-5 **Non atteint au SHA lu (test à écrire)** : aucun événement journalisé ne porte de chaîne hors identifiants hexadécimaux, digests, chemins relatifs à la racine, compteurs, mots d'une liste fermée de catalogue et chaînes à règle de formation nommée (`repository`, `branch`, `phases`, `executor`, `actor`, `events-v0.md:30-32`, `phases-v0.md:155`). Validation : test de catalogue de `tests-et-evaluation.md` § 4.3.1, écrit puis vu rouge (un `title_digest` remplacé par le titre) puis vert ; au SHA lu la propriété n'est tenue que par la revue de `events-v0.md`.
- CA-6 **Non atteint au SHA lu (test à écrire)** : le stderr de `wsd` et la sortie de `ws-journal-verify` sur une campagne ne contiennent aucun texte de fixture (titre, brief, critère, note, résumé, raison, sortie de terminal). Validation : test de redaction de `tests-et-evaluation.md` § 4.3.2, écrit puis vu rouge (un titre injecté dans un message) puis vert.
- CA-7 Le cockpit refuse une requête `POST` sans `Origin` propre (403 `request.origin_refused`), avec `Sec-Fetch-Site` autre que `same-origin` (403 `request.cross_site`), sans session (403 `session.required`) ou sans jeton CSRF (403 `session.csrf_refused`), et aucune page ne contient le marqueur du terminal. Validation : `crates/work-supervision-cockpit/tests/cockpit.rs` ; assertion e2e de la CSP servie depuis un navigateur réel à ajouter (I-37).
- CA-8 Un processus d'un autre UID est refusé sur `run/wsd.sock` (`socket.peer_refused`) et aucun port réseau n'est ouvert par `wsd`. Validation : `a_peer_of_another_uid_is_refused` ; relevé `lsof -i -P` à vide pendant un test de bout en bout (à ajouter).
- CA-9 `ws hook HARNESS` n'écrit rien sur stdout et sort 0 pour tout payload, y compris malformé, et transmet le SHA-256 de l'identifiant de session, jamais l'identifiant. Validation : `crates/work-supervision-cli/tests/hook.rs` (`translate` seulement : payload sans identifiant et harness inconnu refusés par code, `:109-130`) ; `crates/work-supervision-cli/tests/cli.rs:538-640` (stdout vide, sortie 0, identifiant absent du journal) ; **aucun test ne soumet un payload malformé à `ws hook`** : cas à ajouter (§ 4.3.4) ; vecteurs réels de hooks de `tests-et-evaluation.md` § 4.3.4.
- CA-10 La campagne B′ simulée rend 10/10 acceptées, journal valide, ancre `matched`, `ws rebuild --check` égal, 0 worktree, 0 fichier hors racine, dépôt, ancre et entrées du test. Validation : `tests/b_prime_simulation.rs` ; la sortie porte le mot « SIMULATION » et les missions `"simulation": true`.
- CA-11 Dix missions réelles consécutives sont acceptées avec un profil d'exécuteur réel conforme à CA-3 et CA-15 (§ 1.5), sans fichier de passation hors de l'outil, journal valide et 0 worktree orphelin après la dixième ; cette conjonction est la **première moitié** de B′ (`docs/adr/0042-product-vision-and-realization-method.md:78-80`) : la seconde, « une première utilisation réelle de la boucle documentaire par le propriétaire », est portée par `personal-knowledge-workspace` (`specs/personal-knowledge-workspace/prd.md:52`, extraits P02 → P04), pas par ce projet. Validation : dossier de preuve B′ de `tests-et-evaluation.md` § 2.2.1 ; fiche `b-prime` passée de `pending` à atteint par PR portant ce dossier ; `sed -n 41p project.v1.yaml` → `status: "pending"` au SHA lu. **Non atteignable au SHA lu.**
- CA-12 `bun run check` est vert sur la branche servie et le run CI `Code validation` du head l'est aussi. Validation : `gh run list -R libre-ai/ai-work-supervision --limit 1 --json conclusion` → `success` ; 12 contrôles enchaînés par `package.json:48` (`sed -n 48p package.json \| tr '&' '\n' \| grep -c 'bun run'` → 12).
- CA-13 La licence effective du dépôt est EUPL-1.2 pour le code de première partie (preuves : `LICENSE` et résolution GitHub ; l'annotation `aggregate` de `REUSE.toml:3-7` porte MIT et n'est **pas** une preuve d'EUPL-1.2 ; l'annotation EUPL-1.2 `closest` est `REUSE.toml:9-14`), Apache-2.0 ou CC-BY-4.0 là où `LICENSING.md` le prévoit. Validation : `head -1 LICENSE` ≠ « MIT License » ; `gh api repos/libre-ai/ai-work-supervision --jq .license.spdx_id` → `EUPL-1.2` ; `reuse lint` vert (non rejoué dans cette mission : aucune exécution, `[NON VÉRIFIÉ]`) ; `ls LICENSES` sans `MIT.txt`. **Non atteint au SHA lu (E10) ; correction hors mission par PR séparée (HANDOFF-phase-0 §7).**
- CA-14 **Mesure, sans critère de passage** tant que Q9 n'a pas fixé de seuil : le coût machine d'ouverture à 10 000 entrées (temps de vérification, temps de rejeu, taille), alimenté aussi par un journal produit par le pont (CA-18), est mesuré et publié avant toute levée de la dérogation I-09. Validation : test ignoré par défaut de `tests-et-evaluation.md` § 4.3.5, sortie jointe au dossier de preuve ; la levée de la dérogation compare cette sortie au seuil de Q9 une fois fixé.

- CA-15 Le dépôt embarque la bibliothèque d'attestation d'`execution-sandbox` et un run de ce dépôt produit une attestation `harness-attestation.v1` stockée, que vérifie un opérateur sans accès au run (reprend CA-16 de `specs/execution-sandbox/cahier-des-charges.md:232`). Validation : `grep -rl libre-ai-harness .clones/ai-work-supervision` ≥ 1 ; `grep -n 'harness-attestation' specs/ai-work-supervision/cahier-des-charges.md` ≥ 1 ; journal de mission portant le digest de l'attestation. **Non atteint au SHA lu** (`grep -rl libre-ai-harness .clones/ai-work-supervision` → 0, rejoué).
- CA-16 **Non atteint (test à écrire)** : après effacement d'un blob nommé par le journal, `ws-journal-verify` rend `valid` et `ws rebuild --check` rend un verdict défini (refus nommé `BlobMissing` à ce `seq`, ou reconstruction tolérante selon Q8), jamais un panic ni une projection silencieusement incomplète. Validation : test de `tests-et-evaluation.md` § 4.3.6 ; résultat attendu écrit avant le test.
- CA-17 **Non atteint (test à écrire)** : un second processus du même UID qui réécrit journal et ancre de façon cohérente obtient `valid` de `ws-journal-verify` et `matched` de l'ancre : la limite d'ENF-1 est documentée par un test, pas supposée. Validation : test de `tests-et-evaluation.md` § 4.3.7 ; le résultat attendu `valid` y est écrit comme une limite connue.
- CA-18 **Non atteint (test à écrire)** : pour N événements `PreToolUse` identiques reçus par `ws hook`, le nombre d'entrées `session.reported` journalisées est établi par un test et comparé à ce qu'I-33 (`docs/decisions/INVARIANTS.md:39`) tolère ; l'écart éventuel est consigné en Q13. Validation : test de `tests-et-evaluation.md` § 4.3.8 ; R-11 et CA-14 mesurent aussi un journal alimenté par le pont.

- CA-19 **Non atteint (test à écrire)** (ajouté au recoupement de seconde famille du 2026-10-15) Aucun profil d'exécuteur autre que `fake` n'est ajouté tant qu'un test de contrat n'établit pas qu'aucune chaîne dérivée d'un contenu non fiable (brief figé, critères, artefacts) n'entre dans le contexte d'un modèle détenant un outil à effet, sauf en référence opaque (I-32). Prérequis de CA-3 et de CA-11 ; remplace la mention « hors périmètre v0 » du § 8.2. Validation : vecteurs rouges de provenance ; `grep -n 'I-32' specs/ai-work-supervision/*.md` renvoie ce critère.

## 10. Critères d'abandon

Les horizons ci-dessous sont des comptes de commits ou d'exécutions observables, **proposés** et à fixer par le propriétaire (Q12) ; aucune date de calendrier n'est employée.

- AB-1 Condition : la fabrique n'a pas servi à son propre jalon. Validation : au prochain relevé, `git -C .clones/execution-sandbox rev-list --count d1ffdd2bb2f2c8398861c041cf622dfb78a42e64..HEAD` ≥ 100 (seuil proposé, Q12) **et** `grep -rl libre-ai-harness .clones/ai-work-supervision` → 0 **et** `sed -n 41p .clones/ai-work-supervision/project.v1.yaml` → `status: "pending"`, tandis que le propriétaire a déclaré, par une PR sur `project.v1.yaml` de ce dépôt, que dix missions réelles ont été conduites dans un autre outil avec passations (lieu de la source : cette PR et le dossier qu'elle cite) ; l'horizon et la forme de cette déclaration sont à fixer par Q12.
- AB-2 Condition : l'outil n'a pas d'usage propre vérifié (I-19). Validation : Q1 tranchée en option A (outil de la fabrique, dogfooding seul) **et** dans le journal de la racine du propriétaire (hors dépôt, I-21), `grep '"kind":"mission.created"' JOURNAL | grep -vc '"executor":"fake"'` → 0 alors que `git -C .clones/ai-work-supervision rev-list --count 582f21e1a314c412b6cc856cbfe22736d2419889..HEAD` ≥ 50 (seuil proposé, Q12) ; des exécutions vertes de `tests/b_prime_simulation.rs` ne comptent pas, car ce test tourne à chaque `cargo test --workspace` (`b-prime-rehearsal.md:9-11`).
- AB-3 Condition : le journal ou la projection est contourné. Validation : `grep -rnE 'Journal::open|\.append_entry\(|journal\.append\(' crates --include='*.rs' | grep -vE '/tests/|^crates/work-supervision-(journal|store)/|^[^:]+:[0-9]+:\s*//' | wc -l` → 0 au SHA lu (rejoué : 0) et > 0 déclenche l'abandon ; les écritures passent par `Supervisor::append` (`crates/work-supervision-store/src/supervisor.rs:210-216`), appelé par `Supervisor::execute` (`:196-201`) et par les crates `worktree` et `daemon` ; test d'architecture à ajouter (§ 4.3.9) qui rougit si le motif apparaît.
- AB-4 Condition : un commun libre maintenu couvre les quatre propriétés d'oracle. Validation : expérience nommée dans `prd.md` § 7 (tableau « propriété d'oracle × commun », cinq candidats) : pour un candidat, les quatre cellules passent de `[NON VÉRIFIÉ]` à « oui » avec source (`gh api repos/ORG/DEPOT --jq .license.spdx_id` dans une licence admise par ADR-0042 §4, journal vérifiable hors base, reprise après crash prouvée, critères rouges à la base, acceptation à l'octet) ; alors la ligne devient « intégration » (CADRAGE §1.6, `CADRAGE.md:77-82`). Au SHA lu, le benchmark déclaré a pour axe la surface de plugins et exclut le confinement (`ecosystem/portfolio.v1.yaml:66-89`, `parity_axis`), et aucun relevé n'a été fait.

## 11. Dépendances

### 11.1 Internes libre-ai

La colonne « Déclaré / importé » distingue la déclaration (`overrides` de `package.json:10-23`) de l'import réel : `for p in data web-platform testing contracts ui rgpd-kit contracts-authority envelope policy-core-ref; do grep -rhoE "from [\"']@libre-ai/$p[\"'/]" apps packages adapters tools | wc -l; done` → 11, 7, 7, 3, 0, 0, 0, 0, 0.

| Dépôt | Nature | Déclaré / importé | Source |
| --- | --- | --- | --- |
| `project-governance` | gabarit de flotte (`validate-composition.yml@b0a5ff0c`), outillage `@libre-ai/governance`, doctrine ADR-0042 et 2026-09-29, design `docs/architecture/forge/` | déclaré ; workflow | `.github/workflows/code-validation.yml:13` ; `project.v1.yaml:23-26` |
| `schemas-and-contracts` | `@libre-ai/contracts-authority`, `sdk-ts` (`contracts`), `envelope` pour le code récupéré ; contrats `missions-api.v3` (locked, `contracts/STATUS.md:122, 169`), `execution-authorization-v4` (candidate, `:101`), `harness-profile-v3` (candidate, `:112`), `tool-invocation-observation-v1` (candidate, `:159`) ; `execution-plan-body` et `orchestrator-event` cités par `docs/apps/missions.md` et `docs/apps/agent-board.md` ; **aucun contrat consommé par le cœur Rust v0** | `contracts` : importé (3) ; `contracts-authority`, `envelope` : déclarés, 0 import | `package.json:13-15` |
| `application-development-toolkit` | `ui`, `web-platform`, `testing` pour `apps/*` | `web-platform` 7, `testing` 7 importés ; `ui` déclaré, 0 import | `package.json:16-18` |
| `organization-data-lifecycle` | `data`, `rgpd-kit` pour `apps/*` | `data` 11 importé ; `rgpd-kit` déclaré, 0 import | `package.json:19-20` |
| `ai-model-policy` | `policy-core-ref` (déclaré, 0 import) ; **cycle de déclarations** : il déclare `@libre-ai/auth-web` de ce dépôt (cycle E22), sans que ce dépôt importe `policy-core-ref` (`specs/ai-model-policy/prd.md:70`) | déclaré des deux côtés ; aucun import côté ce dépôt | `package.json:21` ; `synthese.md:100-113` |
| `execution-sandbox` | attestation `harness-attestation.v1` et bibliothèque `libre-ai-harness` (attendues, CA-15) ; « verdict C0 » sans artefact (§ 1.5) | aucun | ADR-0042 §1 ; `specs/execution-sandbox/cahier-des-charges.md:232` |
| `execution-continuity-evaluator` | évaluateur pur des observations d'outil, après acceptation du contrat candidat ; **attend** de ce projet un adaptateur `RunControlState` / `StartPreflight` et le rejeu de 10 missions (`specs/execution-continuity-evaluator/prd.md`, ligne H1 : `grep -n '^| H1' specs/execution-continuity-evaluator/prd.md`) ; familles partagées `execution-plan-body`, `orchestrator-event`, `execution-authorization` (`grep -n 'execution-plan-body\|orchestrator-event' specs/execution-continuity-evaluator/cahier-des-charges.md`) | aucun | ADR-0046, `docs/adr/0046-*.md` en-tête |

### 11.1 bis Attentes croisées et chevauchements nommés (sans demande de fusion)

| Projet | Ce qu'il attend ou recoupe | Où c'est traité ici |
| --- | --- | --- |
| `execution-sandbox` | CA-16 de ce projet-là : ce dépôt embarque `libre-ai-harness` et un run produit une attestation | CA-15 ; § 1.5 ; Q11 |
| `execution-continuity-evaluator` | H1 : adaptateur d'état de run et rejeu de 10 missions ; familles `execution-plan-body`, `orchestrator-event` | § 11.1 ; `architecture.md` § 2.2 ; Q6 |
| `artifact-verification` | S3 : lignée de contribution signée « par Work Supervision » (décisions, preuves, écarts de `ws report`) ; fonction non inscrite dans ce cahier | `prd.md` § 10 ; Q6 |
| `ai-model-policy` | H5 : contrat d'appel entre la supervision et l'évaluateur d'éligibilité ; le cycle est un cycle de déclarations | § 11.1 ; RK-3 |
| `database-policy-inspector` | H2 : les 15 fichiers `.sql` de ce dépôt servent de corpus | `prd.md` § 10 |
| `capability-authorization` | matrice d'autorisation ; moitié applicative `apps/missions/src/authz/mission-authorization.ts:1-7`, rangée ici dans le code gelé | Q5 ; `prd.md` § 10 |
| `signalement`, `pi-evidence`, `personal-knowledge-workspace` | capture (P15), format de preuve, seconde moitié de B′ | `prd.md` § 10 ; Q6 ; CA-11 |

### 11.2 Externes

- Rust : `rusqlite` 0.40.2 (bundled), `portable-pty` 0.9.0, `vt100` 0.16.2, `ratatui` 0.30.2,
  `nix` 0.28.0, `rustix` 1.1.5, `tiny_http` 0.12.0, `serde_jcs` 0.2.0, `sha2` 0.11.0
  (`Cargo.toml:27-39`) ; toutes dans la liste `allow` de `deny.toml`.
- Système : `git` installé, appelé en processus séparé (ADR-0042 §10) ; `/bin/sh` comme `SHELL`
  du PTY (`pty-v0.md:12-14`).
- Bun 1.4, TypeScript 7.0.2, Biome 2.5.3, React 19.2.7 pour le code récupéré
  (`package.json:6-31`, `apps/missions/package.json`).
- PGlite 0.5.4 pour la vérification PostgreSQL (`crates/work-supervision-store/postgres-check/package.json`).
- Harness : Claude Code 2.1.295, Codex CLI 0.160.1, Pi 0.84.2 lus le 2026-10-09 ; interfaces
  « change between versions » (`harness-adapters-v0.md:17-21`).

## 12. Risques

| # | Risque | Observable | Réponse |
| --- | --- | --- | --- |
| RK-1 | B′ reste `pending` parce que C0 dépend d'un autre dépôt dont l'avancement déclaré est 0 % (`execution-sandbox/README.md`, section générée) et dont aucun artefact de verdict n'existe (§ 1.5) | fiche `b-prime` inchangée ; AB-1 ; CA-15 | questions Q1, Q7 et Q11 ; critère de fin de `tests-et-evaluation.md` § 6 |
| RK-2 | L'acteur déclaratif permet à tout processus du compte de décider à la place du propriétaire | `coordination-v0.md:16-23` | question Q5 ; CA-8 ne couvre que l'UID |
| RK-3 | Trois générations de code dans un arbre : cycle de **déclarations** `@libre-ai/auth-web` ↔ `policy-core-ref` / `application-development-toolkit` (E22) ; l'import réel côté ce dépôt est nul pour `policy-core-ref` (§ 11.1) ; charge de `bun run check` sur du code gelé | `synthese.md:100-113` ; `package.json:10-23, 48` | question Q3 (R10) |
| RK-4 | Licence effective MIT contre doctrine EUPL (E10) | `LICENSE:1` ; résolution GitHub | CA-13 ; question Q2 ; PR hors mission |
| RK-5 | Format de journal local, non publié : un vérificateur tiers ne peut pas être qualifié sans le code | `journal-v0.md:7-8` | question Q6 ; corpus adverse § 4.3.3 |
| RK-6 | Pont de harness tenu par des payloads écrits à la main, jamais rejoués dans Pi | `harness-adapters-v0.md:17-21, 110` | CA-9 ; vecteurs réels |
| RK-7 | Coût machine des gardes et du rejeu non mesuré | `coordination-v0.md:163-167` ; `journal-v0.md:113` | CA-14 |
| RK-8 | Fiche périmée (`current_situation` au snapshot récupéré) : le gate de flotte lit une situation fausse | `project.v1.yaml:15, 31` | PR de fiche hors mission ; signalé dans `prd.md` § Devenir |
| RK-9 | Ambiguïté « Forge » / « harness » : deux dépôts se réclament du mot (E11) | `LEXICON.md:151-155` | question Q4 |
| RK-10 | Digest d'un texte personnel effacé persistant dans la chaîne ; blob effacé rendant `ws rebuild` impossible à ce `seq` (`BlobMissing`) | `events-v0.md:13-17` ; `projection.rs:121-126` | question Q8 ; CA-16 |
| RK-11 | Non-répudiation attendue par un public NIS2 / DORA mais non fournie : chaîne sans clé, ancre optionnelle du même compte | `journal-v0.md:29-37` ; `daemon-v0.md:87-101` | ENF-1 ; CA-17 ; Q5 |
| RK-12 | Flux d'événements du pont (`PreToolUse` / `PostToolUse` en `working`) horodaté par appel dans le journal, au regard d'I-33 ; coût de journal mesuré sans ce flux | `hook.rs:74` ; `INVARIANTS.md:39` | § 8.3 ; CA-18 ; Q13 |

## 13. Table de conformité à SPECIFICATION-STANDARD

Les 13 sections obligatoires de `docs/specifications/SPECIFICATION-STANDARD.md` (CADRAGE §2.10,
`CADRAGE.md:109-116`) et l'endroit où chacune est traitée dans ces cinq fichiers.

| # | Section du standard | Traitée dans | Absente : raison |
| --- | --- | --- | --- |
| 1 | Purpose and actors | `cahier-des-charges.md` § 2, § 3 ; `prd.md` § 1-2 | — |
| 2 | Journeys | `prd.md` § 4 (scénarios) ; `architecture.md` § 3 (états) | — |
| 3 | Non-goals | `cahier-des-charges.md` § 4.2 | — |
| 4 | Domain protocol | `architecture.md` § 2 (contrats), § 3 (états), événements `events-v0.md` | — |
| 5 | Refusal matrix | `architecture.md` § 4 (codes stables) | — |
| 6 | Data | `architecture.md` § 5 (journal, blobs, projection, rétention) | clé tenant absente : dérogation à I-09 **proposée** par ADR-0042 §5 (`proposed`, donc hypothèse et non contrainte, CADRAGE §2.8 ; `0042…:138-145`), non acceptée ; question Q9 ; migration source = journal |
| 7 | Authentication and authorization | `cahier-des-charges.md` § 8.1 ; `architecture.md` § 6 (permissions minimales) | Biscuit et RLS absents de la v0 : outil local mono-utilisateur, socket UID ; cookie du cockpit `HttpOnly; SameSite=Strict` sans `Secure` ni `__Host-` (standard : cookie opaque `HttpOnly` `Secure`, I-09), justifié seulement par la boucle locale en HTTP, écart en § 8.4 ; le code récupéré `auth-web` et `apps/missions` (RLS, `missions-v1.datalog`) sont la référence gelée de la v1 (`apps/missions/AGENTS.md:5-7`) |
| 8 | Runtime boundaries | `architecture.md` § 1 (composants) | — ; Bun/React n'intervient que dans le code gelé |
| 9 | Accessibility and degraded mode | `architecture.md` § 7 (mode dégradé) | aucune exigence d'accessibilité (clavier, technologies d'assistance, mouvement réduit) n'est écrite ni testée pour le cockpit (HTML sans script, formulaires natifs) ni pour la TUI `ws attach` ; la v0 est un outil de ligne de commande et de TUI d'un seul utilisateur, ce qui n'est pas une raison d'absence : question Q10 |
| 10 | Contracts | `architecture.md` § 2 | aucun contrat JSON Schema / OpenAPI / WIT publié pour le cœur v0 ; format de journal local (`journal-v0.md:7-8`) — question Q6 |
| 11 | Evidence | `tests-et-evaluation.md` § 5 | smoke absent : aucun déploiement |
| 12 | Work packages | `tests-et-evaluation.md` § 4.3 (neuf tests à ajouter, dépendances ordonnées) ; § 9 (CA-15 à CA-18) | le standard exige propriétaire de flux, ordre de dépendance et chemins disjoints ; ce document donne l'ordre (CA-16 avant toute suppression de blob ; CA-15 avant CA-11) mais pas de découpe par propriétaire de flux : le dépôt porte son propre découpage par outline (`phases-v0.md`, `coordination-v0.md:218-272`), non repris ici faute de décision Q1 |
| 13 | Release and rollback | `architecture.md` § 8 (reconstructibilité), § 9 (sortie) | aucune release : `publish = false` (`Cargo.toml:25`), binaires construits localement ; rollback = `ws rebuild` et `state.sqlite.previous` (`daemon-v0.md:71-74`) |

Enveloppe commune : le protocole `wsd` emploie `{data, meta}` / `{error}` (`daemon-v0.md:40-41`) ;
**écart** au standard : pas de `message` public, de `requestId` ni de `retryable` dans l'enveloppe
d'erreur du socket local (standard `:34, 40-43` ; `architecture.md` § 4) ; le cockpit sert du HTML,
pas du JSON. Protocole de mutation : révision
optimiste tenue (`mission.revision_stale`) ; `commandId` et idempotence par identifiant de commande
**absents** (les commandes sont des opérations sur socket local, une par connexion) — écart consigné
pour la v1.
