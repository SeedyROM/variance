import { describe, it, expect, beforeEach } from "vitest";
import { useAppStore } from "../appStore";
import type { NodeStatus } from "../appStore";

beforeEach(() => {
  useAppStore.setState({
    nodeStatus: "idle",
    apiPort: null,
    error: null,
    wsConnected: false,
  });
});

describe("initial state", () => {
  it("starts with idle nodeStatus", () => {
    expect(useAppStore.getState().nodeStatus).toBe("idle");
  });

  it("starts with null apiPort", () => {
    expect(useAppStore.getState().apiPort).toBeNull();
  });

  it("starts with null error", () => {
    expect(useAppStore.getState().error).toBeNull();
  });

  it("starts with wsConnected=false", () => {
    expect(useAppStore.getState().wsConnected).toBe(false);
  });
});

describe("setNodeStatus", () => {
  it("transitions through all valid states", () => {
    const states: NodeStatus[] = [
      "idle",
      "starting",
      "running",
      "stopping",
      "error",
      "needs-unlock",
    ];
    for (const status of states) {
      useAppStore.getState().setNodeStatus(status);
      expect(useAppStore.getState().nodeStatus).toBe(status);
    }
  });
});

describe("setApiPort", () => {
  it("sets a numeric port", () => {
    useAppStore.getState().setApiPort(9000);
    expect(useAppStore.getState().apiPort).toBe(9000);
  });

  it("can be reset to null", () => {
    useAppStore.getState().setApiPort(9000);
    useAppStore.getState().setApiPort(null);
    expect(useAppStore.getState().apiPort).toBeNull();
  });
});

describe("setError", () => {
  it("sets an error message", () => {
    useAppStore.getState().setError("Connection failed");
    expect(useAppStore.getState().error).toBe("Connection failed");
  });

  it("clears an error", () => {
    useAppStore.getState().setError("Connection failed");
    useAppStore.getState().setError(null);
    expect(useAppStore.getState().error).toBeNull();
  });
});

describe("setWsConnected", () => {
  it("sets connected to true", () => {
    useAppStore.getState().setWsConnected(true);
    expect(useAppStore.getState().wsConnected).toBe(true);
  });

  it("sets connected back to false", () => {
    useAppStore.getState().setWsConnected(true);
    useAppStore.getState().setWsConnected(false);
    expect(useAppStore.getState().wsConnected).toBe(false);
  });
});
