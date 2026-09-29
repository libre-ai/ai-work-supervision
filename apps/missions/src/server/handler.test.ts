import { describe, expect, test } from "bun:test";
import { createMissionsHandler } from "./handler";

const handler = createMissionsHandler(() => "req_0000000000000000");

describe("missions cockpit handler", () => {
  test("assigns distinct opaque problem identifiers when no generator is injected", async () => {
    const defaultHandler = createMissionsHandler();
    const first = await defaultHandler(new Request("https://missions.test/missing"));
    const second = await defaultHandler(new Request("https://missions.test/missing"));
    const firstId = (await first.json()).error.requestId;
    const secondId = (await second.json()).error.requestId;
    expect(first.status).toBe(404);
    expect(second.status).toBe(404);
    expect(firstId).toMatch(/^req_[a-f0-9]{32}$/);
    expect(secondId).toMatch(/^req_[a-f0-9]{32}$/);
    expect(firstId).not.toBe(secondId);
  });

  test("serves the server-rendered cockpit at /", async () => {
    const response = await handler(new Request("https://missions.test/"));
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toContain("text/html");
    const html = await response.text();
    expect(html).toContain("Missions");
    expect(html).toContain("<caption>");
  });

  test("reports health as JSON", async () => {
    const response = await handler(new Request("https://missions.test/api/health"));
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({
      service: "libre-ai-missions",
      status: "ok",
      version: "v1",
    });
  });

  test("an unknown route is not found", async () => {
    const response = await handler(new Request("https://missions.test/nope"));
    expect(response.status).toBe(404);
  });
});
