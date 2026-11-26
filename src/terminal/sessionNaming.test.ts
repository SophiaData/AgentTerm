import { describe, expect, it } from "vitest";
import {
  createPromptLineTracker,
  deriveAutoTitleFromPrompt,
  normalizeManualTitle,
} from "./sessionNaming";

describe("session naming helpers", () => {
  it("normalizes and validates manual session titles", () => {
    expect(normalizeManualTitle("  排查   订单表慢查询  ")).toEqual({
      ok: true,
      title: "排查 订单表慢查询",
    });
    expect(normalizeManualTitle("   ").ok).toBe(false);
    expect(normalizeManualTitle("第一行\n第二行").ok).toBe(false);
    expect(normalizeManualTitle("字".repeat(61)).ok).toBe(false);
  });

  it("derives concise automatic titles from meaningful prompts", () => {
    expect(deriveAutoTitleFromPrompt("修复 Windows 首页地址乱码")).toBe(
      "修复 Windows 首页地址乱码",
    );
    expect(
      deriveAutoTitleFromPrompt(
        "帮我分析一下今天广告转化表为什么多出了重复主键并且给一个修复方案",
      ),
    ).toBe("帮我分析一下今天广告转化表为什么多出了重复主键…");
  });

  it("ignores slash commands, trivial confirmations and secret-like inputs", () => {
    expect(deriveAutoTitleFromPrompt("/exit")).toBeNull();
    expect(deriveAutoTitleFromPrompt("/model gpt-5")).toBeNull();
    expect(deriveAutoTitleFromPrompt("y")).toBeNull();
    expect(deriveAutoTitleFromPrompt("yes")).toBeNull();
    expect(deriveAutoTitleFromPrompt("???")).toBeNull();
    expect(deriveAutoTitleFromPrompt("token=fixture-value")).toBeNull();
    expect(
      deriveAutoTitleFromPrompt("AWS_SECRET_ACCESS_KEY=fixture-value"),
    ).toBeNull();
    expect(deriveAutoTitleFromPrompt("sk-1234567890abcdef")).toBeNull();
    expect(deriveAutoTitleFromPrompt("Bearer eyJhbGciOi...")).toBeNull();
    expect(deriveAutoTitleFromPrompt("Secretary: 整理会议纪要并发布")).toBe(
      "Secretary: 整理会议纪要并发布",
    );
  });

  it("tracks terminal input lines across escape sequences, backspaces and bracketed paste", () => {
    const tracker = createPromptLineTracker();
    expect(tracker.feed("\x1b[I\x1b[A\x1b[B")).toEqual([]);
    expect(tracker.feed("排查日")).toEqual([]);
    expect(tracker.feed("错\x7f志\r")).toEqual(["排查日志"]);
    expect(tracker.feed("不要这行\x03")).toEqual([]);
    expect(tracker.feed("\x1b[200~核对 ODPS 分区\x1b[201~\r\n")).toEqual([
      "核对 ODPS 分区",
    ]);
  });
});
