import { createRequestHandler, renderSsrDocument } from "@libre-ai/web-platform";
import { missionsCockpitDocument } from "../shared/document";
import { COCKPIT_FIXTURE } from "../ui/fixture";

// The Missions cockpit request handler. The read view is server-rendered from
// a contract fixture (the spec's runtime boundary: no real mission or
// orchestrator integration until a bounded work package and conformance review
// are approved). No client assets are served — the view works without
// JavaScript.
export function createMissionsHandler(
  requestId: (request: Request) => string = () => `req_${crypto.randomUUID().replaceAll("-", "")}`,
): (request: Request) => Promise<Response> {
  return createRequestHandler({
    requestId,
    routes: {
      "/": () => renderSsrDocument(missionsCockpitDocument(COCKPIT_FIXTURE)),
      "/api/health": () =>
        Response.json({ service: "libre-ai-missions", status: "ok", version: "v1" }),
    },
  });
}
