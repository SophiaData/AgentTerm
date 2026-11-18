import { describe, expect, it } from "vitest";

import { advance, byteLength, sliceChunkAfter } from "./offsetStitch";

describe("sliceChunkAfter", () => {
  it("drops a chunk entirely inside the snapshot", () => {
    // Snapshot already covered bytes 0..10; this chunk ends at 10.
    expect(sliceChunkAfter({ data: "hello", endOffset: 5 }, 10)).toBe("");
    expect(sliceChunkAfter({ data: "abcde", endOffset: 10 }, 10)).toBe("");
  });

  it("writes a chunk entirely after the snapshot", () => {
    // Snapshot at offset 5; chunk covers 5..10 — all new.
    expect(sliceChunkAfter({ data: "world", endOffset: 10 }, 5)).toBe("world");
  });

  it("writes only the tail when a chunk straddles the boundary", () => {
    // Snapshot at offset 5; chunk covers 0..10 ("helloworld") — keep "world".
    expect(sliceChunkAfter({ data: "helloworld", endOffset: 10 }, 5)).toBe(
      "world",
    );
  });

  it("handles multibyte UTF-8 by byte offset, not char count", () => {
    // "你" is 3 bytes. Chunk "你好" is 6 bytes ending at 6.
    const chunk = { data: "你好", endOffset: 6 };
    expect(byteLength("你好")).toBe(6);
    // Snapshot already wrote the first 3 bytes ("你"); we must show only "好".
    expect(sliceChunkAfter(chunk, 3)).toBe("好");
    // Fully covered.
    expect(sliceChunkAfter(chunk, 6)).toBe("");
  });

  it("never splits a multibyte char mid-sequence", () => {
    // Snapshot at 3 bytes = after full "你"; boundary is clean.
    const out = sliceChunkAfter({ data: "你好", endOffset: 6 }, 3);
    expect(out).toBe("好");
    // Decoded result must be valid text, not a replacement character.
    expect(out).not.toContain("\uFFFD");
  });

  it("rewriting sequentially reproduces the full stream exactly once", () => {
    // Simulate: snapshot covers "hello", then live chunks " world" and "!".
    let written = byteLength("hello"); // 5
    const chunks = [
      { data: " world", endOffset: 11 },
      { data: "!", endOffset: 12 },
    ];
    let shown = "hello";
    for (const c of chunks) {
      shown += sliceChunkAfter(c, written);
      written = c.endOffset;
    }
    expect(shown).toBe("hello world!");
  });

  it("does not duplicate a chunk that the snapshot already contains", () => {
    // Snapshot already includes "abc" (offset 3). A live event for the same
    // "abc" arrives late (endOffset 3). It must be dropped, not appended.
    let shown = "abc";
    const written = 3;
    const lateChunk = { data: "abc", endOffset: 3 };
    shown += sliceChunkAfter(lateChunk, written);
    expect(shown).toBe("abc");
  });
});

describe("advance (shared offset state update)", () => {
  // Regression for #11: replaying several already-covered cached events after a
  // snapshot must not roll the offset back and re-display content.
  it("does not re-display covered events and keeps offset monotonic (#11)", () => {
    // Snapshot "abcdef" ends at offset 6.
    let shown = "abcdef";
    let offset = 6;

    const steps: {
      chunk: { data: string; endOffset: number };
      expectOffset: number;
    }[] = [
      { chunk: { data: "abc", endOffset: 3 }, expectOffset: 6 }, // fully covered
      { chunk: { data: "def", endOffset: 6 }, expectOffset: 6 }, // fully covered
      { chunk: { data: "ghi", endOffset: 9 }, expectOffset: 9 }, // new
    ];

    for (const step of steps) {
      const result = advance(step.chunk, offset);
      shown += result.text;
      offset = result.offset;
      expect(offset).toBe(step.expectOffset);
    }

    expect(shown).toBe("abcdefghi");
  });

  it("a late duplicate never appends and never lowers the offset", () => {
    const offset = 6;
    const result = advance({ data: "def", endOffset: 6 }, offset);
    expect(result.text).toBe("");
    expect(result.offset).toBe(6);
    // Offset cannot go backwards even if an older event arrives.
    const older = advance({ data: "abc", endOffset: 3 }, result.offset);
    expect(older.text).toBe("");
    expect(older.offset).toBe(6);
  });

  it("writes a straddling chunk tail and advances to its end offset", () => {
    const result = advance({ data: "helloworld", endOffset: 10 }, 5);
    expect(result.text).toBe("world");
    expect(result.offset).toBe(10);
  });
});
