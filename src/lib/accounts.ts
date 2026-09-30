// What the account form accepts. It mirrors the backend's rules so a mistake is explained before
// anything starts; the backend checks again and has the last word.

import type { AccountsStatus, ServerStatus } from "../api/types";
import { CHECKING } from "./summary";

/** Left out of a generated password: characters that look like each other in the game's font. */
const PASSWORD_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";

/** Characters the console or the command templates would read specially. */
const PASSWORD_FORBIDDEN = "\"'\\`{}";

export const GENERATED_PASSWORD_LENGTH = 12;

/** A whole number below `limit` from the browser's cryptographic random source, without bias. */
function secureInt(limit: number): number {
  const range = 0x1_0000_0000;
  const cutoff = range - (range % limit);
  const buffer = new Uint32Array(1);
  for (;;) {
    crypto.getRandomValues(buffer);
    if (buffer[0] < cutoff) return buffer[0] % limit;
  }
}

/** A random password that always passes [`passwordProblem`]. `randomInt` is replaceable for tests. */
export function generatePassword(length = GENERATED_PASSWORD_LENGTH, randomInt: (limit: number) => number = secureInt): string {
  let out = "";
  for (let i = 0; i < length; i += 1) out += PASSWORD_ALPHABET[randomInt(PASSWORD_ALPHABET.length)];
  return out;
}

export function nameProblem(name: string, status: Pick<AccountsStatus, "nameMax" | "botPrefix">): string | null {
  if (name === "") return "Enter an account name.";
  if ([...name].length > status.nameMax) return `The account name can have at most ${status.nameMax} characters.`;
  if (!/^[A-Za-z0-9]+$/.test(name)) return "The account name can only use the letters A to Z and the digits 0 to 9.";
  if (name.toUpperCase().startsWith(status.botPrefix)) return `Names starting with ${status.botPrefix.toLowerCase()} belong to the playerbots. Choose another.`;
  return null;
}

/** There is no minimum length: the server takes any password, and this is the person's own server. */
export function passwordProblem(password: string, status: Pick<AccountsStatus, "passwordMax">): string | null {
  const chars = [...password];
  if (chars.length === 0) return "Enter a password.";
  if (chars.length > status.passwordMax) return `The password can have at most ${status.passwordMax} characters; the game client cannot type more.`;
  const plain = chars.every((c) => {
    const code = c.codePointAt(0) ?? 0;
    return code >= 0x21 && code <= 0x7e && !PASSWORD_FORBIDDEN.includes(c);
  });
  if (!plain) return "The password can use letters, digits and symbols, but no spaces, quotes, backslashes or braces.";
  return null;
}

export interface AccountForm {
  name: string;
  password: string;
  /** A new account, or a new password for the account named. */
  mode: "create" | "password";
}

/**
 * Why the form cannot be sent, or null when it can. A new account needs only a name: with no
 * password typed, the account's own name is its password (the backend does that). Changing a
 * password needs one.
 */
export function formProblem(form: AccountForm, status: AccountsStatus): string | null {
  if (form.mode === "create") return nameProblem(form.name, status) ?? (form.password === "" ? null : passwordProblem(form.password, status));
  return passwordProblem(form.password, status);
}

/** What is shown, briefly, once an account has been created. */
export function accountCreatedText(name: string, passwordIsName: boolean): string {
  const upper = name.toUpperCase();
  return passwordIsName ? `Account ${upper} created. Its password is its name.` : `Account ${upper} created.`;
}

/** Why an account cannot be created or changed right now, or null. Console commands need the world server. */
export function accountsBlocked(accounts: AccountsStatus | null, server: ServerStatus | null): string | null {
  if (!accounts || !server) return CHECKING;
  if (accounts.state !== "ready") return accounts.note ?? "The accounts cannot be read.";
  if (server.state !== "running") return "Start the servers first: accounts are managed through the world server's console.";
  return null;
}

const GM_WORD = ["Player", "Moderator", "Game master", "Administrator"];

/** The word for a game-master level. */
export function gmWord(level: number): string {
  return GM_WORD[level] ?? `Level ${level}`;
}

/** A date from the database, or a dash when it has none. */
export function formatWhen(seconds: number | null, locale?: string): string {
  return seconds === null ? "—" : new Date(seconds * 1000).toLocaleDateString(locale);
}
