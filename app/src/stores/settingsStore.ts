import { create } from "zustand";
import { persist } from "zustand/middleware";

export type Theme = "light" | "dark" | "system";

interface SettingsStore {
  tabSize: 2 | 4;
  setTabSize: (size: 2 | 4) => void;
  theme: Theme;
  setTheme: (theme: Theme) => void;
}

export const useSettingsStore = create<SettingsStore>()(
  persist(
    (set) => ({
      tabSize: 4,
      setTabSize: (tabSize) => set({ tabSize }),
      theme: "system" as Theme,
      setTheme: (theme) => set({ theme }),
    }),
    { name: "variance-settings" }
  )
);
