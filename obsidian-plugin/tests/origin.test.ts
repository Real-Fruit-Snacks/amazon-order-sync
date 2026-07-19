import { describe, it, expect } from "vitest";
import { isAllowedOrigin } from "../main.ts";

describe("isAllowedOrigin", () => {
  it("allows amazon.com and its subdomains", () => {
    expect(isAllowedOrigin("https://www.amazon.com")).toBe(true);
    expect(isAllowedOrigin("https://amazon.com")).toBe(true);
    expect(isAllowedOrigin("https://smile.amazon.com")).toBe(true);
    expect(isAllowedOrigin("http://www.amazon.com")).toBe(true);
  });

  it("allows requests with no Origin (non-browser local tooling)", () => {
    expect(isAllowedOrigin(undefined)).toBe(true);
    expect(isAllowedOrigin("")).toBe(true);
  });

  it("rejects arbitrary websites", () => {
    expect(isAllowedOrigin("https://evil.example.com")).toBe(false);
    expect(isAllowedOrigin("https://attacker.io")).toBe(false);
    expect(isAllowedOrigin("null")).toBe(false);
  });

  it("rejects look-alike hosts that merely contain amazon.com", () => {
    expect(isAllowedOrigin("https://amazon.com.evil.example")).toBe(false);
    expect(isAllowedOrigin("https://notamazon.com")).toBe(false);
    expect(isAllowedOrigin("https://www.amazon.com.attacker.net")).toBe(false);
    expect(isAllowedOrigin("https://evilamazon.com")).toBe(false);
  });

  it("rejects malformed origin values", () => {
    expect(isAllowedOrigin("not a url")).toBe(false);
    expect(isAllowedOrigin("javascript:alert(1)")).toBe(false);
  });
});
