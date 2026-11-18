// Pure helpers for stitching a snapshot together with live PTY chunks using a
// shared byte offset. Kept separate from the React component so the tricky
// ordering can be unit-tested with controlled event sequences.

export interface OffsetChunk {
  data: string;
  /** Total bytes the backend had emitted when this chunk finished. */
  endOffset: number;
}

const encoder = new TextEncoder();

/** Byte length of a JS string when encoded as UTF-8 (matches the backend). */
export function byteLength(text: string): number {
  return encoder.encode(text).length;
}

/**
 * Given a snapshot ending at `snapshotOffset`, decide what part of `chunk` still
 * needs writing. Returns "" when the chunk is fully covered by the snapshot,
 * the chunk unchanged when it lies entirely after it, or the trailing slice
 * (by UTF-8 bytes) when it straddles the boundary.
 *
 * `writtenOffset` is how many bytes have already been shown; pass the snapshot's
 * offset after writing the snapshot, or the previous chunk's endOffset while
 * streaming live.
 */
export function sliceChunkAfter(
  chunk: OffsetChunk,
  writtenOffset: number,
): string {
  if (chunk.endOffset <= writtenOffset) {
    return "";
  }
  const startByte = chunk.endOffset - byteLength(chunk.data);
  const skip = Math.max(0, writtenOffset - startByte);
  if (skip === 0) {
    return chunk.data;
  }
  const bytes = encoder.encode(chunk.data);
  return new TextDecoder().decode(bytes.subarray(skip));
}
