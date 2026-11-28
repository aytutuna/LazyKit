import { describe, expect, it } from "vitest";
import { normalizeEmail } from "../src/account/email";

describe("email addresses", () => {
  it("folds case, width and surrounding space into one spelling", () => {
    const want = "ada@example.com";
    for (const raw of ["Ada@Example.COM", "  ada@example.com ", "ａｄａ@ｅｘａｍｐｌｅ．ｃｏｍ", "ADA@EXAMPLE.com\n"]) expect(normalizeEmail(raw)).toBe(want);
  });

  it("turns an international domain into its ASCII form", () => {
    expect(normalizeEmail("a@bücher.example")).toBe("a@xn--bcher-kva.example");
    expect(normalizeEmail("a@BÜCHER.example")).toBe("a@xn--bcher-kva.example");
  });

  it("keeps lookalike letters apart, since they are different addresses", () => {
    expect(normalizeEmail("аda@example.com")).not.toBe(normalizeEmail("ada@example.com"));
  });

  it("refuses control, invisible, bidi and separator characters", () => {
    for (const raw of ["a​da@example.com", "ada@exam‍ple.com", "a‮da@example.com", "ada\u0000@example.com", "a da@example.com", "ada@exa mple.com", "ada @example.com"]) {
      expect(normalizeEmail(raw), JSON.stringify(raw)).toBeUndefined();
    }
  });

  it("refuses more than one at-sign, quotes, lists and comments", () => {
    for (const raw of ["a@b@example.com", '"a@b"@example.com', "a@example.com,b@example.com", "a@example.com;b@example.com", "ada(comment)@example.com", "<ada@example.com>", "ada@example.com:80"]) {
      expect(normalizeEmail(raw), raw).toBeUndefined();
    }
  });

  it("refuses odd domains and local parts", () => {
    for (const raw of ["ada@localhost", "ada@127.0.0.1", "ada@example.123", "ada@-example.com", "ada@example..com", "ada@.example.com", ".ada@example.com", "ada.@example.com", "a..da@example.com", "@example.com", "ada@", "ada@example.com/path", "ada@example.com?x", "ada@example.com#x"]) {
      expect(normalizeEmail(raw), raw).toBeUndefined();
    }
    expect(normalizeEmail("a".repeat(65) + "@example.com")).toBeUndefined();
    expect(normalizeEmail("a@" + "b".repeat(64) + ".com")).toBeUndefined();
    expect(normalizeEmail("x".repeat(400))).toBeUndefined();
    expect(normalizeEmail(42)).toBeUndefined();
  });

  it("keeps plus tags and dots as part of the address", () => {
    expect(normalizeEmail("a.da+tag@example.com")).toBe("a.da+tag@example.com");
  });
});
