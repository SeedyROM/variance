import { create } from "zustand";
import { persist } from "zustand/middleware";

interface IdentityStore {
  did: string | null;
  verifyingKey: string | null;
  createdAt: string | null;
  identityPath: string | null;
  isOnboarded: boolean;

  setIdentity: (did: string, verifyingKey: string, createdAt: string) => void;
  setIdentityPath: (path: string) => void;
  setIsOnboarded: (value: boolean) => void;
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

      setIdentity: (did, verifyingKey, createdAt) =>
        set({ did, verifyingKey, createdAt }),
      setIdentityPath: (identityPath) => set({ identityPath }),
      setIsOnboarded: (isOnboarded) => set({ isOnboarded }),
      reset: () =>
        set({
          did: null,
          verifyingKey: null,
          createdAt: null,
          identityPath: null,
          isOnboarded: false,
        }),
    }),
    { name: "variance-identity" }
  )
);
