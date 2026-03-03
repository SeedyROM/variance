import { create } from "zustand";

export type NodeStatus = "idle" | "starting" | "running" | "stopping" | "error" | "needs-unlock";

interface AppStore {
  nodeStatus: NodeStatus;
  apiPort: number | null;
  error: string | null;

  setNodeStatus: (status: NodeStatus) => void;
  setApiPort: (port: number | null) => void;
  setError: (error: string | null) => void;
}

export const useAppStore = create<AppStore>((set) => ({
  nodeStatus: "idle",
  apiPort: null,
  error: null,

  setNodeStatus: (nodeStatus) => set({ nodeStatus }),
  setApiPort: (apiPort) => set({ apiPort }),
  setError: (error) => set({ error }),
}));
