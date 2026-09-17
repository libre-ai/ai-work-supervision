import { describe, expect, test } from "bun:test";
import { createSpecificationsHandler } from "./handler";

const handler = createSpecificationsHandler(() => "req_0000000000000000");

describe("specifications cockpit handler", () => {
  test("serves the server-rendered cockpit at /", async () => {
    const response = await handler(new Request("https://specifications.test/"));
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toContain("text/html");
    const html = await response.text();
    expect(html).toContain("Spécifications");
    expect(html).toContain("<caption>");
  });

  test("reports health as JSON", async () => {
    const response = await handler(new Request("https://specifications.test/api/health"));
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({
      service: "libre-ai-specifications",
      status: "ok",
      version: "v1",
    });
  });

  test("an unknown route is not found", async () => {
    const response = await handler(new Request("https://specifications.test/nope"));
    expect(response.status).toBe(404);
  });
});
