// Read-only Specifications cockpit view. Accessibility first: an ordered
// textual/table view; the workspace lifecycle is conveyed as text and never
// relies on colour. Server-rendered and usable without JavaScript — the authoring
// journeys (requirement/contract/review/accept) and the acceptance seam arrive in
// later increments.
//
// WorkspaceState is identity-free (the id lives in persistence, not the fold), so
// the cockpit pairs each state with its workspace id in a small view-model.

import type { Status, WorkspaceState } from "../domain/workspace";

export interface SpecWorkspaceView {
  readonly id: string;
  readonly state: WorkspaceState;
}

const STATUS_LABEL: Readonly<Record<Status, string>> = {
  draft: "Brouillon",
  submitted: "Soumise",
  accepted: "Acceptée",
  superseded: "Remplacée",
};

export function SpecificationsCockpit({
  workspaces,
}: {
  readonly workspaces: readonly SpecWorkspaceView[];
}) {
  return (
    <>
      <a className="skip-link" href="#specifications">
        Aller à la liste des spécifications
      </a>
      <header>
        <h1>Spécifications</h1>
        <p>
          Produire des paquets de spécification acceptés, immuables et adressés par contenu :
          problème, exigences, décisions, contrats, risques, critères d'acceptation et approbations
          indépendantes. L'acceptation exige au moins deux approbateurs distincts.
        </p>
      </header>
      <main id="specifications">
        <h2 id="specifications-heading">Spécifications suivies</h2>
        <p>{`${workspaces.length} spécification(s).`}</p>
        <table aria-labelledby="specifications-heading">
          <caption>
            Liste des spécifications : identifiant, état du cycle de vie, révision, nombre
            d'exigences et nombre d'approbateurs. L'état est indiqué en toutes lettres.
          </caption>
          <thead>
            <tr>
              <th scope="col">Spécification</th>
              <th scope="col">État</th>
              <th scope="col">Révision</th>
              <th scope="col">Exigences</th>
              <th scope="col">Approbateurs</th>
            </tr>
          </thead>
          <tbody>
            {workspaces.map(({ id, state }) => (
              <tr key={id}>
                <th scope="row">{id}</th>
                <td>{STATUS_LABEL[state.status]}</td>
                <td>{state.revision}</td>
                <td>{state.requirementIds.length}</td>
                <td>{state.approverIds.length}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </main>
    </>
  );
}
