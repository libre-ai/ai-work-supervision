import type { DocumentDescriptor } from "@libre-ai/web-platform";
import type { Mission } from "../domain/mission";
import { MissionsCockpit } from "../ui/missions-cockpit";

// The read-only cockpit is server-rendered and works without JavaScript, so no
// client module is declared; interactivity (command journeys, live regions)
// arrives with a later increment.
export function missionsCockpitDocument(missions: readonly Mission[]): DocumentDescriptor {
  return {
    app: <MissionsCockpit missions={missions} />,
    description: "Cockpit humain des missions d'agents bornées de Libre AI.",
    lang: "fr",
    title: "Libre AI — Missions",
  };
}
