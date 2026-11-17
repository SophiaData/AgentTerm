import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import App from "./App";
import { isSupportedTarget, PRODUCT_NAME, SUPPORTED_TARGETS } from "./meta";

describe("AgentTerm application shell", () => {
  it("renders the product foundation screen", () => {
    const html = renderToStaticMarkup(<App />);

    expect(html).toContain(PRODUCT_NAME);
    expect(html).toContain("工程底座已就绪");
  });

  it("declares exactly the macOS arm64 and Windows x64 targets", () => {
    expect(SUPPORTED_TARGETS.map((target) => target.id)).toEqual([
      "macos-arm64",
      "windows-x64",
    ]);
    expect(isSupportedTarget("macos-arm64")).toBe(true);
    expect(isSupportedTarget("macos-x64")).toBe(false);
  });
});
