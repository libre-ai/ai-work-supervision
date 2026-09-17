import { describe, expect, test } from "bun:test";
import { renderStaticDocument } from "@libre-ai/web-platform";
import { specificationsCockpitDocument } from "../shared/document";
import { COCKPIT_FIXTURE } from "./fixture";

// The read view is static (no client module), so the deterministic static render
// is the document the browser receives without JavaScript.
function renderCockpit(): string {
  return new TextDecoder().decode(
    renderStaticDocument(specificationsCockpitDocument(COCKPIT_FIXTURE)),
  );
}

describe("specifications cockpit accessible read view", () => {
  test("renders a well-formed HTML document", async () => {
    const html = renderCockpit();
    expect(html).toStartWith("<!doctype html>");
    expect(html).toContain('lang="fr"');
    expect(html).toContain("Libre AI — Spécifications");
  });

  test("presents an accessible table with a caption and column headers", async () => {
    const html = renderCockpit();
    expect(html).toContain("<caption>");
    expect(html).toContain('scope="col"');
    expect(html).toContain('scope="row"');
    expect(html).toContain("État");
    expect(html).toContain("Révision");
    expect(html).toContain("Exigences");
    expect(html).toContain("Approbateurs");
    // A skip link and a main landmark anchor keyboard navigation.
    expect(html).toContain('href="#specifications"');
    expect(html).toContain('id="specifications"');
  });

  test("conveys the lifecycle as text, never colour alone", async () => {
    const html = renderCockpit();
    // Each status renders its human label.
    expect(html).toContain("Brouillon");
    expect(html).toContain("Soumise");
    expect(html).toContain("Acceptée");
    expect(html).toContain("Remplacée");
    // No inline colour styling is used to carry meaning.
    expect(html).not.toContain("style=");
  });

  test("lists every fixture workspace by id", async () => {
    const html = renderCockpit();
    for (const { id } of COCKPIT_FIXTURE) {
      expect(html).toContain(id);
    }
    expect(html).toContain(`${COCKPIT_FIXTURE.length} spécification(s).`);
  });
});
