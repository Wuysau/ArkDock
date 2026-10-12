/** Human-readable spelling only. Never use this value for identity, trust or file access. */
export function displayPath(path: string): string {
  const prefix = '\\\\?\\';
  if (!path.startsWith(prefix)) return path;
  const remainder = path.slice(prefix.length);
  if (/^[a-z]:\\/i.test(remainder)) return remainder;
  if (/^UNC\\/i.test(remainder)) {
    const networkPath = remainder.slice(4);
    // Preserve server/share identity, including WSL UNC hosts. Device and
    // volume namespaces have no equivalent ordinary path and stay unchanged.
    if (/^[^\\]+\\[^\\]+(?:\\|$)/.test(networkPath)) return `\\\\${networkPath}`;
  }
  return path;
}
