import { create } from "zustand";
import { persist } from "zustand/middleware";

interface IdentityStore {
  did: string | null;
  verifyingKey: string | null;
  createdAt: string | null;
  identityPath: string | null;
  isOnboarded: boolean;
  username: string | null;
  discriminator: number | null;
  displayName: string | null;

  setIdentity: (did: string, verifyingKey: string, createdAt: string) => void;
  setIdentityPath: (path: string) => void;
  setIsOnboarded: (value: boolean) => void;
  setUsername: (username: string, discriminator: number, displayName: string) => void;
  clearUsername: () => void;
  reset: () => void;
}

export const useIdentityStore = create<IdentityStore>()(
  persist(
    (set) => ({
      did: null,
      verifyingKey: null,
      createdAt: null,
      identityPath: null,
      isOnboarded: false,
      username: null,
      discriminator: null,
      displayName: null,

      setIdentity: (did, verifyingKey, createdAt) => set({ did, verifyingKey, createdAt }),
      setIdentityPath: (identityPath) => set({ identityPath }),
      setIsOnboarded: (isOnboarded) => set({ isOnboarded }),
      setUsername: (username, discriminator, displayName) =>
        set({ username, discriminator, displayName }),
      clearUsername: () => set({ username: null, discriminator: null, displayName: null }),
      reset: () =>
        set({
          did: null,
          verifyingKey: null,
          createdAt: null,
          identityPath: null,
          isOnboarded: false,
          username: null,
          discriminator: null,
          displayName: null,
        }),
    }),
    {
      name: "variance-identity",
      // Only persist onboarding + identity path — NOT username/discriminator.
      // Username comes from the backend identity file (unique per data dir).
      // Persisting it here would leak across Tauri instances sharing the same
      // WebView origin (tauri://localhost).
      partialize: (state) => ({
        identityPath: state.identityPath,
        isOnboarded: state.isOnboarded,
      }),
    }
  )
);
