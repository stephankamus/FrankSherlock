import { describe, expect, it } from "vitest";
import { clearToken, resolveToken, saveToken } from "../config";

function memoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() {
      return values.size;
    },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => [...values.keys()][index] ?? null,
    removeItem: (key) => void values.delete(key),
    setItem: (key, value) => void values.set(key, value),
  };
}

describe("resolveToken", () => {
  it("takes the token from the URL and remembers it", () => {
    const storage = memoryStorage();
    expect(resolveToken("?token=abc", storage)).toBe("abc");
    expect(resolveToken("", storage)).toBe("abc");
  });

  it("returns an empty string when nothing was provided", () => {
    expect(resolveToken("", memoryStorage())).toBe("");
  });

  it("stores and clears a token accepted by the server", () => {
    const storage = memoryStorage();
    saveToken("accepted", storage);
    expect(resolveToken("", storage)).toBe("accepted");
    clearToken(storage);
    expect(resolveToken("", storage)).toBe("");
  });
});
