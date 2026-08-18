import { describe, expect, it } from "vitest";
import { formatAtoms, parseCmfd, shortenHash, sumAtoms } from "./amount";

describe("CMFD amount helpers", () => {
  it("parses and formats exact eight-decimal values", () => {
    expect(parseCmfd("1.25000001")).toBe(125_000_001n);
    expect(formatAtoms(125_000_001n)).toBe("1.25000001");
    expect(formatAtoms(-1n, true)).toBe("−0.00000001");
    expect(formatAtoms(1n, true)).toBe("+0.00000001");
  });

  it("preserves values above JavaScript's safe integer range", () => {
    const atoms = "18446744073709551615";
    expect(formatAtoms(atoms)).toBe("184,467,440,737.09551615");
    expect(sumAtoms(atoms, "1")).toBe(18_446_744_073_709_551_616n);
  });

  it("rejects ambiguous or over-precise decimal input", () => {
    expect(parseCmfd("01.0")).toBeNull();
    expect(parseCmfd("1.000000001")).toBeNull();
    expect(parseCmfd("-1")).toBeNull();
    expect(parseCmfd("1e4")).toBeNull();
  });

  it("shortens hashes without changing short values", () => {
    expect(shortenHash("abcd", 2, 2)).toBe("abcd");
    expect(shortenHash("0123456789abcdef", 4, 4)).toBe("0123…cdef");
  });
});
