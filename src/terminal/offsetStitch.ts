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

function validUtf8SuffixStart(bytes: Uint8Array, offset: number): number {
  let index = Math.max(0, Math.min(offset, bytes.length));
  // UTF-8 continuation bytes always match 10xxxxxx (0x80..0xBF); skip past any
  // partial leading byte when slicing inside a multi-byte character.
  while (index < bytes.length && (bytes[index] & 0xc0) === 0x80) {
    index += 1;
  }
  return index;
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
  const safeStart = validUtf8SuffixStart(bytes, skip);
  return new TextDecoder().decode(bytes.subarray(safeStart));
}

/**
 * The one place that decides what to write for a chunk and how far the offset
 * advances. Both the React view and the tests call this, so the offset can never
 * be rolled back in one place and not the other.
 *
 * Invariant: the returned `offset` is monotonic non-decreasing. A chunk that is
 * fully covered by what we've already written contributes "" and must NOT pull
 * the offset backwards (that was bug #11: dropping an old event reassigned the
 * offset to that event's smaller endOffset and let a later event re-display).
 */
export function advance(
  chunk: OffsetChunk,
  writtenOffset: number,
): { text: string; offset: number } {
  const text = sliceChunkAfter(chunk, writtenOffset);
  const offset = Math.max(writtenOffset, chunk.endOffset);
  return { text, offset };
}
