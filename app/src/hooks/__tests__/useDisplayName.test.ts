import { describe, it, expect } from "vitest";
import { useDisplayName, formatPeerName } from "../useDisplayName";

describe("useDisplayName", () => {
  it("returns 'Unknown' for null DID", () => {
    expect(useDisplayName(null)).toBe("Unknown");
  });

  it("returns last 12 chars of DID by default", () => {
    const did = "did:variance:abcdefghijklmnop";
    expect(useDisplayName(did)).toBe(did.slice(-12));
  });

  it("respects custom fallbackChars", () => {
    const did = "did:variance:abcdef";
    expect(useDisplayName(did, 6)).toBe("abcdef");
  });

  it("returns full DID when shorter than fallbackChars", () => {
    const did = "short";
    expect(useDisplayName(did)).toBe("short");
  });
});

describe("formatPeerName", () => {
  it("returns displayName when provided", () => {
    expect(formatPeerName("did:variance:xyz", "alice#0042")).toBe("alice#0042");
  });

  it("returns truncated DID when displayName is null", () => {
    const did = "did:variance:abcdefghijklmnop";
    expect(formatPeerName(did, null)).toBe(did.slice(-12));
  });

  it("returns truncated DID when displayName is undefined", () => {
    const did = "did:variance:abcdefghijklmnop";
    expect(formatPeerName(did)).toBe(did.slice(-12));
  });

  it("returns truncated DID when displayName is empty string", () => {
    const did = "did:variance:abcdefghijklmnop";
    // Empty string is falsy, so falls back to truncation
    expect(formatPeerName(did, "")).toBe(did.slice(-12));
  });

  it("respects custom fallbackChars", () => {
    const did = "did:variance:abcdefghijklmnop";
    expect(formatPeerName(did, null, 8)).toBe(did.slice(-8));
  });
});
