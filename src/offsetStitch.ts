/** Byte-based, monotonic stitching of Rust PTY snapshots and live output. */
export interface OffsetChunk {
  data: string;
  endOffset: number;
}

const encoder = new TextEncoder();

export function byteLength(text: string): number {
  return encoder.encode(text).length;
}

/** Return only UTF-8 text not already covered by the last committed byte offset. */
export function sliceChunkAfter(
  chunk: OffsetChunk,
  writtenOffset: number,
): string {
  if (chunk.endOffset <= writtenOffset) return "";
  const bytes = encoder.encode(chunk.data);
  const startByte = chunk.endOffset - bytes.length;
  let skip = Math.max(0, writtenOffset - startByte);
  if (skip === 0) return chunk.data;
  // If a producer supplied an offset inside a code point, skip the incomplete
  // leading sequence rather than emitting the Unicode replacement character.
  while (skip < bytes.length && (bytes[skip] & 0xc0) === 0x80) skip++;
  return new TextDecoder().decode(bytes.subarray(skip));
}

/** A stale/duplicate event must never roll back the committed byte offset. */
export function advance(
  chunk: OffsetChunk,
  writtenOffset: number,
): { text: string; offset: number } {
  return {
    text: sliceChunkAfter(chunk, writtenOffset),
    offset: Math.max(writtenOffset, chunk.endOffset),
  };
}
