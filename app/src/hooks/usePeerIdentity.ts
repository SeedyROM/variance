import { useQuery } from "@tanstack/react-query";
import { identityApi } from "../api/client";

/**
 * Hook to resolve a DID to its username/display_name
 * Uses React Query for caching and automatic refetching
 */
export function usePeerIdentity(did: string | null) {
  return useQuery({
    queryKey: ["identity", did],
    queryFn: async () => {
      if (!did) return null;
      try {
        const resolved = await identityApi.resolve(did);
        return resolved;
      } catch {
        return null;
      }
    },
    enabled: !!did,
    staleTime: 5 * 60 * 1000, // 5 minutes
    gcTime: 30 * 60 * 1000, // 30 minutes (formerly cacheTime)
  });
}

/**
 * Returns formatted display name for a peer
 * Tries to use username#discriminator, falls back to truncated DID
 */
export function usePeerDisplayName(did: string | null, fallbackChars = 12): string {
  const { data } = usePeerIdentity(did);

  if (!did) return "Unknown";

  // If we have a resolved identity with username info, format it
  if (data?.verifying_key) {
    // Note: The backend's ResolvedIdentity type doesn't include username yet
    // For now, fall back to DID truncation
    // TODO: Update ResolvedIdentity type to include username/discriminator
    return did.slice(-fallbackChars);
  }

  return did.slice(-fallbackChars);
}
