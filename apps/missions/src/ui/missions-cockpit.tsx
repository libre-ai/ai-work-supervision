// Read-only Missions cockpit view. Accessibility first (docs/apps/missions.md
// §Accessibility): an ordered textual/table view; state, risk and verdict are
// conveyed as text and never rely on colour. Server-rendered and usable without
// JavaScript — the command journeys (propose/approve/…) and live announcements
// arrive in later increments.

import type { Mission, MissionState, RiskLevel } from "../domain/mission";

const STATE_LABEL: Readonly<Record<MissionState, string>> = {
  proposed: "Proposée",
  assessed: "Risque évalué",
  approved: "Approuvée",
  refused: "Refusée",
  running: "En cours",
  blocked: "Bloquée",
  paused: "En pause",
  cancelled: "Annulée",
  "result-submitted": "Résultat soumis",
  accepted: "Validée",
  rejected: "Résultat rejeté",
  abandoned: "Abandonnée",
};

const RISK_LABEL: Readonly<Record<RiskLevel, string>> = {
  low: "faible",
  medium: "moyen",
  high: "élevé",
  critical: "critique",
};

function riskText(mission: Mission): string {
  return mission.risk ? RISK_LABEL[mission.risk.level] : "non évalué";
}

export function MissionsCockpit({ missions }: { readonly missions: readonly Mission[] }) {
  return (
    <>
      <a className="skip-link" href="#missions">
        Aller à la liste des missions
      </a>
      <header>
        <h1>Missions</h1>
        <p>
          Proposer, évaluer, observer et valider des missions d'agents bornées. L'activité rapportée
          reste distincte du résultat validé.
        </p>
      </header>
      <main id="missions">
        <h2 id="missions-heading">Missions suivies</h2>
        <p>{`${missions.length} mission(s).`}</p>
        <table aria-labelledby="missions-heading">
          <caption>
            Liste des missions : identifiant, état, niveau de risque et révision. L'état et le
            risque sont indiqués en toutes lettres.
          </caption>
          <thead>
            <tr>
              <th scope="col">Mission</th>
              <th scope="col">État</th>
              <th scope="col">Risque</th>
              <th scope="col">Révision</th>
            </tr>
          </thead>
          <tbody>
            {missions.map((mission) => (
              <tr key={mission.id}>
                <th scope="row">{mission.id}</th>
                <td>{STATE_LABEL[mission.state]}</td>
                <td>{riskText(mission)}</td>
                <td>{mission.revision}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </main>
    </>
  );
}
