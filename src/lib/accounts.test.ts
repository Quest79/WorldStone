import { describe, expect, it } from "vitest";
import type { AccountsStatus, ServerStatus } from "../api/types";
import { accountCreatedText, accountsBlocked, formatWhen, formProblem, generatePassword, gmWord, nameProblem, passwordProblem } from "./accounts";

const status = (extra: Partial<AccountsStatus> = {}): AccountsStatus => ({
  state: "ready",
  note: null,
  accounts: [],
  botAccounts: 16,
  botPrefix: "RNDBOT",
  maxGmLevel: 3,
  nameMax: 16,
  passwordMax: 16,
  ...extra,
});

const server = (state: ServerStatus["state"]): ServerStatus => ({ state, note: null, host: "127.0.0.1", services: [], restartNeeded: false, heldBy: [] });

describe("nameProblem", () => {
  it("accepts letters and digits", () => {
    expect(nameProblem("Paul01", status())).toBeNull();
    expect(nameProblem("a".repeat(16), status())).toBeNull();
  });

  it("explains what is wrong", () => {
    expect(nameProblem("", status())).toContain("Enter");
    expect(nameProblem("a".repeat(17), status())).toContain("at most 16");
    expect(nameProblem("a b", status())).toContain("letters");
    expect(nameProblem("x;y", status())).toContain("letters");
    expect(nameProblem("élan", status())).toContain("letters");
  });

  it("keeps the names the bots use for the bots, in any letter case", () => {
    expect(nameProblem("rndbot7", status())).toContain("playerbots");
    expect(nameProblem("RndBotFan", status())).toContain("playerbots");
    expect(nameProblem("robot", status())).toBeNull();
  });
});

describe("passwordProblem", () => {
  it("accepts visible characters", () => {
    expect(passwordProblem("Str0ng!pw", status())).toBeNull();
    expect(passwordProblem("a".repeat(16), status())).toBeNull();
  });

  it("has no minimum length, only a maximum by characters", () => {
    expect(passwordProblem("x", status())).toBeNull();
    expect(passwordProblem("p".repeat(17), status())).toContain("at most 16");
  });

  it("is needed when one is being set", () => {
    expect(passwordProblem("", status())).toContain("Enter a password");
  });

  it("refuses what the console or the command templates would read specially", () => {
    for (const bad of ["has space1", 'quote"ed1', "it's-one", "back\\slash", "brace{1}xx", "tick`xx1", "tab\tulator", "línea123"]) {
      expect(passwordProblem(bad, status()), bad).toContain("no spaces");
    }
  });
});

describe("generatePassword", () => {
  it("draws every character from the random source and stays within what is accepted", () => {
    let calls = 0;
    const password = generatePassword(12, (limit) => {
      calls += 1;
      return (calls * 7) % limit;
    });
    expect(calls).toBe(12);
    expect(password).toHaveLength(12);
    expect(passwordProblem(password, status())).toBeNull();
  });

  it("is different each time with the real random source, and never has look-alike characters", () => {
    const seen = new Set<string>();
    for (let i = 0; i < 50; i += 1) {
      const password = generatePassword();
      expect(password).toHaveLength(12);
      expect(password).not.toMatch(/[01OIl]/);
      expect(passwordProblem(password, status())).toBeNull();
      seen.add(password);
    }
    expect(seen.size).toBe(50);
  });
});

describe("formProblem", () => {
  it("checks the name only when an account is being created", () => {
    expect(formProblem({ name: "", password: "Str0ng!pw", mode: "create" }, status())).toContain("Enter");
    expect(formProblem({ name: "", password: "Str0ng!pw", mode: "password" }, status())).toBeNull();
    expect(formProblem({ name: "PAUL", password: "", mode: "password" }, status())).toContain("Enter a password");
    expect(formProblem({ name: "PAUL", password: "x", mode: "password" }, status())).toBeNull();
    expect(formProblem({ name: "PAUL", password: "Str0ng!pw", mode: "create" }, status())).toBeNull();
  });

  it("needs nothing but a name to create an account", () => {
    expect(formProblem({ name: "PAUL", password: "", mode: "create" }, status())).toBeNull();
    expect(formProblem({ name: "PAUL", password: "1", mode: "create" }, status())).toBeNull();
    expect(formProblem({ name: "", password: "", mode: "create" }, status())).toContain("Enter an account name");
  });

  it("still refuses a typed password the console could not take", () => {
    expect(formProblem({ name: "PAUL", password: "has space", mode: "create" }, status())).toContain("no spaces");
  });
});

describe("accountCreatedText", () => {
  it("names the account in capitals, as the game does, and says when the name is the password", () => {
    expect(accountCreatedText("paul", false)).toBe("Account PAUL created.");
    expect(accountCreatedText("paul", true)).toBe("Account PAUL created. Its password is its name.");
  });
});

describe("accountsBlocked", () => {
  it("waits for both answers", () => {
    expect(accountsBlocked(null, server("running"))).toBe("Checking…");
    expect(accountsBlocked(status(), null)).toBe("Checking…");
  });

  it("passes on why the accounts cannot be read", () => {
    expect(accountsBlocked(status({ state: "blocked", note: "The database is not running." }), server("running"))).toBe("The database is not running.");
    expect(accountsBlocked(status({ state: "unknown", note: null }), server("running"))).toContain("cannot be read");
  });

  it("needs the servers up, because the console lives in the world server", () => {
    expect(accountsBlocked(status(), server("stopped"))).toContain("Start the servers");
    expect(accountsBlocked(status(), server("starting"))).toContain("Start the servers");
    expect(accountsBlocked(status(), server("running"))).toBeNull();
  });
});

describe("labels", () => {
  it("names the game-master levels and dates", () => {
    expect(gmWord(0)).toBe("Player");
    expect(gmWord(3)).toBe("Administrator");
    expect(gmWord(7)).toBe("Level 7");
    expect(formatWhen(null)).toBe("—");
    expect(formatWhen(1_790_670_072, "en-US")).toMatch(/2026/);
  });
});
