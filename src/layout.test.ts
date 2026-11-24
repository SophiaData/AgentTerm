import { describe, expect, it } from "vitest";
import { parseStoredLayout } from "./layout";

describe("layout persistence", () => {
  it("accepts valid version two layout values", () => {
    expect(
      parseStoredLayout(
        JSON.stringify({
          version: 2,
          sidebarWidth: 220,
          sideWidth: 420,
          filesOpen: true,
          projectId: "E:\\AgentTerm",
        }),
      ),
    ).toEqual({
      sidebarWidth: 220,
      sideWidth: 420,
      filesOpen: true,
      projectId: "E:\\AgentTerm",
    });
  });

  it.each([
    "{broken",
    '{"version":1}',
    '{"version":2,"sidebarWidth":-1,"sideWidth":420,"filesOpen":true,"projectId":null}',
    '{"version":2,"sidebarWidth":220,"sideWidth":"wide","filesOpen":true,"projectId":null}',
    '{"version":2,"sidebarWidth":220,"sideWidth":420,"filesOpen":"yes","projectId":null}',
    '{"version":2,"sidebarWidth":220,"sideWidth":420,"filesOpen":true,"projectId":42}',
  ])("rejects invalid stored layout %s", (value) => {
    expect(parseStoredLayout(value)).toBeNull();
  });
});
