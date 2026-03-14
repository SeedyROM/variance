import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";
import { useToastStore } from "../toastStore";

beforeEach(() => {
  vi.useFakeTimers();
  useToastStore.setState({ toasts: [] });
});

afterEach(() => {
  vi.useRealTimers();
});

describe("initial state", () => {
  it("starts with empty toasts", () => {
    expect(useToastStore.getState().toasts).toEqual([]);
  });
});

describe("addToast", () => {
  it("adds a toast with default error variant", () => {
    useToastStore.getState().addToast("Something broke");
    const toasts = useToastStore.getState().toasts;
    expect(toasts).toHaveLength(1);
    expect(toasts[0].message).toBe("Something broke");
    expect(toasts[0].variant).toBe("error");
  });

  it("adds a toast with explicit variant", () => {
    useToastStore.getState().addToast("Saved!", "success");
    const toasts = useToastStore.getState().toasts;
    expect(toasts).toHaveLength(1);
    expect(toasts[0].variant).toBe("success");
  });

  it("adds a toast with info variant", () => {
    useToastStore.getState().addToast("FYI", "info");
    expect(useToastStore.getState().toasts[0].variant).toBe("info");
  });

  it("assigns unique IDs to each toast", () => {
    useToastStore.getState().addToast("first");
    useToastStore.getState().addToast("second");
    const toasts = useToastStore.getState().toasts;
    expect(toasts).toHaveLength(2);
    expect(toasts[0].id).not.toBe(toasts[1].id);
  });

  it("auto-removes toast after 4 seconds", () => {
    useToastStore.getState().addToast("temporary");
    expect(useToastStore.getState().toasts).toHaveLength(1);
    vi.advanceTimersByTime(4_000);
    expect(useToastStore.getState().toasts).toHaveLength(0);
  });

  it("does not auto-remove before 4 seconds", () => {
    useToastStore.getState().addToast("still here");
    vi.advanceTimersByTime(3_999);
    expect(useToastStore.getState().toasts).toHaveLength(1);
  });

  it("removes only the expired toast, not others", () => {
    useToastStore.getState().addToast("first");
    vi.advanceTimersByTime(2_000);
    useToastStore.getState().addToast("second");
    vi.advanceTimersByTime(2_000);
    // "first" should be gone (4s elapsed), "second" still present (2s elapsed)
    const toasts = useToastStore.getState().toasts;
    expect(toasts).toHaveLength(1);
    expect(toasts[0].message).toBe("second");
  });
});

describe("removeToast", () => {
  it("removes a toast by id", () => {
    useToastStore.getState().addToast("to remove");
    const id = useToastStore.getState().toasts[0].id;
    useToastStore.getState().removeToast(id);
    expect(useToastStore.getState().toasts).toHaveLength(0);
  });

  it("does nothing for non-existent id", () => {
    useToastStore.getState().addToast("keep me");
    useToastStore.getState().removeToast(999999);
    expect(useToastStore.getState().toasts).toHaveLength(1);
  });
});
