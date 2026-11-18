import { describe, expect, it } from "vitest";
import { newSessionId, sessionTitle, type SessionInfo } from "./types";

const SESSION: SessionInfo = {
  id: "session-1",
  toolId: "claude",
  toolName: "Claude Code",
  projectPath: "/work/agent-term",
};

describe("session helpers", () => {
  it("titles a session with the project folder and tool", () => {
    expect(sessionTitle(SESSION)).toBe("agent-term · Claude Code");
  });

  it("generates unique session ids", () => {
    const first = newSessionId();
    const second = newSessionId();

    expect(first).toMatch(/^session-/);
    expect(second).toMatch(/^session-/);
    expect(first).not.toBe(second);
  });
});
