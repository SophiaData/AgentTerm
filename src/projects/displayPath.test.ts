import { describe, expect, it } from "vitest";
import { displayPath } from "./displayPath";

describe("displayPath", () => {
  it.each([
    [String.raw`\\?\E:\数据 项目`, String.raw`E:\数据 项目`],
    ["\\\\?\\C:\\", "C:\\"],
    [String.raw`\\?\UNC\server\共享\项目`, String.raw`\\server\共享\项目`],
    [String.raw`\\?\unc\server\share`, String.raw`\\server\share`],
  ])("renders the Windows directory %s as %s", (path, expected) => {
    expect(displayPath(path)).toBe(expected);
  });

  it.each([
    String.raw`E:\数据 项目`,
    String.raw`\\server\共享\项目`,
    "/Users/example/中文项目",
    "/tmp/\\?\\filename",
    String.raw`\\.\pipe\service`,
    String.raw`\\?\Volume{example}\项目`,
    String.raw`\\?\UNC\server`,
    String.raw`\\?\C:relative`,
    "",
  ])("preserves other paths: %s", (path) => {
    expect(displayPath(path)).toBe(path);
  });
});
