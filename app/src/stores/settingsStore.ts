import { create } from "zustand";
import { persist } from "zustand/middleware";

export type Theme = "light" | "dark" | "system";

/** Sidebar width constraints (px). */
export const SIDEBAR_MIN_WIDTH = 220;
export const SIDEBAR_MAX_WIDTH = 480;
export const SIDEBAR_DEFAULT_WIDTH = 288; // Tailwind w-72

interface SettingsStore {
  tabSize: 2 | 4;
  setTabSize: (size: 2 | 4) => void;
  theme: Theme;
  setTheme: (theme: Theme) => void;
  sidebarWidth: number;
  setSidebarWidth: (width: number) => void;
}

export const useSettingsStore = create<SettingsStore>()(
  persist(
    (set) => ({
      tabSize: 4,
      setTabSize: (tabSize) => set({ tabSize }),
      theme: "system" as Theme,
      setTheme: (theme) => set({ theme }),
      sidebarWidth: SIDEBAR_DEFAULT_WIDTH,
      setSidebarWidth: (sidebarWidth) => set({ sidebarWidth }),
    }),
    { name: "variance-settings" }
  )
);
