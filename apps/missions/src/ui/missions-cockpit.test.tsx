import { describe, expect, test } from "bun:test";
import { renderStaticDocument } from "@libre-ai/web-platform";
import { missionsCockpitDocument } from "../shared/document";
import { COCKPIT_FIXTURE } from "./fixture";

// The read view is static (no client module), so the deterministic static
// render is the document the browser receives without JavaScript.
function renderCockpit(): string {
  return new TextDecoder().decode(renderStaticDocument(missionsCockpitDocument(COCKPIT_FIXTURE)));
}

describe("missions cockpit accessible read view", () => {
  test("renders a well-formed HTML document", async () => {
    const html = renderCockpit();
    expect(html).toStartWith("<!doctype html>");
    expect(html).toContain('lang="fr"');
    expect(html).toContain("Libre AI — Missions");
  });

  test("presents an accessible table with a caption and column headers", async () => {
    const html = renderCockpit();
    expect(html).toContain("<caption>");
    expect(html).toContain('scope="col"');
    expect(html).toContain('scope="row"');
    expect(html).toContain("État");
    expect(html).toContain("Risque");
    // A skip link and a main landmark anchor keyboard navigation.
    expect(html).toContain('href="#missions"');
    expect(html).toContain('id="missions"');
  });

  test("conveys state and risk as text, never colour alone", async () => {
    const html = renderCockpit();
    // Every fixture state renders its human label.
    expect(html).toContain("Proposée");
    expect(html).toContain("Approuvée");
    expect(html).toContain("En cours");
    expect(html).toContain("Validée");
    // Risk levels render as words, and an unassessed mission says so.
    expect(html).toContain("moyen");
    expect(html).toContain("élevé");
    expect(html).toContain("non évalué");
    // No inline colour styling is used to carry meaning.
    expect(html).not.toContain("style=");
  });

  test("lists every fixture mission by id", async () => {
    const html = renderCockpit();
    for (const mission of COCKPIT_FIXTURE) {
      expect(html).toContain(mission.id);
    }
    expect(html).toContain(`${COCKPIT_FIXTURE.length} mission(s).`);
  });
});
