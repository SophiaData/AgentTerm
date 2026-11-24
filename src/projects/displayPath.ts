// Presentation only: keep the original path for identity, storage and launching.
export function displayPath(path: string): string {
  if (/^\\\\\?\\[A-Za-z]:\\/.test(path)) {
    return path.slice(4);
  }
  if (/^\\\\\?\\UNC\\[^\\]+\\[^\\]+(?:\\|$)/i.test(path)) {
    return `\\\\${path.slice(8)}`;
  }
  return path;
}
