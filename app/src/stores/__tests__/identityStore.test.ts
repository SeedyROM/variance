import { describe, it, expect, beforeEach } from "vitest";
import { useIdentityStore } from "../identityStore";

beforeEach(() => {
  useIdentityStore.setState({
    did: null,
    verifyingKey: null,
    createdAt: null,
    identityPath: null,
    isOnboarded: false,
    username: null,
    discriminator: null,
    displayName: null,
  });
});

describe("initial state", () => {
  it("starts with null did and displayName", () => {
    const { did, displayName } = useIdentityStore.getState();
    expect(did).toBeNull();
    expect(displayName).toBeNull();
  });
});

describe("setIdentity", () => {
  it("persists did and verifyingKey", () => {
    useIdentityStore.getState().setIdentity("did:variance:test", "vkey123", "2024-01-01");
    const { did, verifyingKey, createdAt } = useIdentityStore.getState();
    expect(did).toBe("did:variance:test");
    expect(verifyingKey).toBe("vkey123");
    expect(createdAt).toBe("2024-01-01");
  });
});

describe("setUsername", () => {
  it("stores username, discriminator, and displayName", () => {
    useIdentityStore.getState().setUsername("alice", 42, "alice#0042");
    const { username, discriminator, displayName } = useIdentityStore.getState();
    expect(username).toBe("alice");
    expect(discriminator).toBe(42);
    expect(displayName).toBe("alice#0042");
  });
});

describe("clearUsername", () => {
  it("resets username fields to null", () => {
    useIdentityStore.getState().setUsername("alice", 42, "alice#0042");
    useIdentityStore.getState().clearUsername();
    const { username, discriminator, displayName } = useIdentityStore.getState();
    expect(username).toBeNull();
    expect(discriminator).toBeNull();
    expect(displayName).toBeNull();
  });
});
