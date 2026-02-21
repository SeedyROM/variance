/**
 * Returns a human-friendly display name for a DID.
 * Falls back to the last N chars of the DID.
 */
export function useDisplayName(did: string | null, fallbackChars = 12): string {
  if (!did) return "Unknown";
  return `${did.slice(-fallbackChars)}`;
}

/**
 * Returns a formatted display name: username#disc if available, or truncated DID.
 * `displayName` is the full name#disc string from the identity API.
 */
export function formatPeerName(
  did: string,
  displayName?: string | null,
  fallbackChars = 12
): string {
  if (displayName) return displayName;
  return did.slice(-fallbackChars);
}
