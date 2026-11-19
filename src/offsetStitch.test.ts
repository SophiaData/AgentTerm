import { describe, expect, it } from "vitest";
import { advance, byteLength, sliceChunkAfter } from "./offsetStitch";

describe("PTY byte-offset stitching (ported legacy regressions)", () => {
  it("drops chunks already inside snapshot", () => {
    expect(sliceChunkAfter({ data: "hello", endOffset: 5 }, 10)).toBe("");
    expect(sliceChunkAfter({ data: "abcde", endOffset: 10 }, 10)).toBe("");
  });
  it("appends a chunk strictly after snapshot", () => {
    expect(sliceChunkAfter({ data: "world", endOffset: 10 }, 5)).toBe("world");
  });
  it("keeps only the uncovered tail of overlapping output", () => {
    expect(sliceChunkAfter({ data: "helloworld", endOffset: 10 }, 5)).toBe(
      "world",
    );
    expect(advance({ data: "helloworld", endOffset: 10 }, 5)).toEqual({
      text: "world",
      offset: 10,
    });
  });
  it("counts UTF-8 bytes instead of JS code units", () => {
    expect(byteLength("你好")).toBe(6);
    expect(sliceChunkAfter({ data: "你好", endOffset: 6 }, 3)).toBe("好");
    expect(sliceChunkAfter({ data: "你好", endOffset: 6 }, 6)).toBe("");
  });
  it("does not emit replacement characters at code point boundary", () => {
    const text = sliceChunkAfter({ data: "你好", endOffset: 6 }, 3);
    expect(text).toBe("好");
    expect(text).not.toContain("\uFFFD");
  });
  it("skips incomplete code points when offset falls inside one", () => {
    expect(sliceChunkAfter({ data: "你好", endOffset: 6 }, 4)).toBe("");
  });
  it("reproduces sequential stream exactly once", () => {
    let offset = byteLength("hello");
    let shown = "hello";
    for (const chunk of [
      { data: " world", endOffset: 11 },
      { data: "!", endOffset: 12 },
    ]) {
      const next = advance(chunk, offset);
      shown += next.text;
      offset = next.offset;
    }
    expect(shown).toBe("hello world!");
  });
  it("ignores duplicates already covered by snapshot", () => {
    let shown = "abc";
    shown += sliceChunkAfter({ data: "abc", endOffset: 3 }, 3);
    expect(shown).toBe("abc");
  });
  it("never rolls offset backwards on stale cached events (legacy #11)", () => {
    let shown = "abcdef";
    let offset = 6;
    for (const [chunk, expected] of [
      [{ data: "abc", endOffset: 3 }, 6],
      [{ data: "def", endOffset: 6 }, 6],
      [{ data: "ghi", endOffset: 9 }, 9],
    ] as const) {
      const next = advance(chunk, offset);
      shown += next.text;
      offset = next.offset;
      expect(offset).toBe(expected);
    }
    expect(shown).toBe("abcdefghi");
    expect(advance({ data: "abc", endOffset: 3 }, offset)).toEqual({
      text: "",
      offset: 9,
    });
  });
});
