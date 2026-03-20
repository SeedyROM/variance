import { create } from "zustand";

export type NodeStatus = "idle" | "starting" | "running" | "stopping" | "error" | "needs-unlock";

export type SettingsSection = "account" | "network" | "storage" | "appearance";

interface AppStore {
  nodeStatus: NodeStatus;
  apiPort: number | null;
  error: string | null;
  wsConnected: boolean;
  showSettings: boolean;
  settingsSection: SettingsSection;

  setNodeStatus: (status: NodeStatus) => void;
  setApiPort: (port: number | null) => void;
  setError: (error: string | null) => void;
  setWsConnected: (connected: boolean) => void;
  openSettings: (section?: SettingsSection) => void;
  closeSettings: () => void;
  setSettingsSection: (section: SettingsSection) => void;
}

export const useAppStore = create<AppStore>((set) => ({
  nodeStatus: "idle",
  apiPort: null,
  error: null,
  wsConnected: false,
  showSettings: false,
  settingsSection: "account",

  setNodeStatus: (nodeStatus) => set({ nodeStatus }),
  setApiPort: (apiPort) => set({ apiPort }),
  setError: (error) => set({ error }),
  setWsConnected: (wsConnected) => set({ wsConnected }),
  openSettings: (section) => set({ showSettings: true, ...(section && { settingsSection: section }) }),
  closeSettings: () => set({ showSettings: false }),
  setSettingsSection: (settingsSection) => set({ settingsSection }),
}));
