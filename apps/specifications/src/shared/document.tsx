import type { DocumentDescriptor } from "@libre-ai/web-platform";
import type { SpecWorkspaceView } from "../ui/specifications-cockpit";
import { SpecificationsCockpit } from "../ui/specifications-cockpit";

// The read-only cockpit is server-rendered and works without JavaScript, so no
// client module is declared; interactivity (authoring journeys, live regions)
// arrives with a later increment.
export function specificationsCockpitDocument(
  workspaces: readonly SpecWorkspaceView[],
): DocumentDescriptor {
  return {
    app: <SpecificationsCockpit workspaces={workspaces} />,
    description: "Cockpit humain des paquets de spécification acceptés de Libre AI.",
    lang: "fr",
    title: "Libre AI — Spécifications",
  };
}
