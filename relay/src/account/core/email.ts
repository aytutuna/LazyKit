// One address, one spelling: the account is named by this, so lookalike forms must collapse.
export function normalizeEmail(raw: unknown): string | undefined {
  if (typeof raw !== "string" || raw.length > 320) return undefined;
  const s = raw.trim().normalize("NFKC").toLowerCase().normalize("NFKC");
  if (/[\p{C}\p{Z}<>()[\]\\,;:"]/u.test(s)) return undefined;
  const parts = s.split("@");
  if (parts.length !== 2) return undefined;
  const [local, domain] = parts;
  if (local.length === 0 || local.length > 64 || !/^[\p{L}\p{N}._%+'!#$&*/=?^`{|}~-]+$/u.test(local)) return undefined;
  if (local.startsWith(".") || local.endsWith(".") || local.includes("..")) return undefined;
  if (!/^[\p{L}\p{N}.-]+$/u.test(domain) || !domain.includes(".")) return undefined;
  let host: string;
  try {
    host = new URL(`http://${domain}`).hostname;
  } catch {
    return undefined;
  }
  const labels = host.split(".");
  if (host.length > 253 || labels.length < 2 || labels.some((l) => !/^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/.test(l)) || /^\d+$/.test(labels[labels.length - 1])) return undefined;
  return `${local}@${host}`;
}

export interface Mail {
  to: string;
  subject: string;
  text: string;
}

export interface Mailer {
  send(mail: Mail): Promise<void>;
}

export class MailNotConfiguredError extends Error {
  name = "MailNotConfiguredError";
}

export class MailFailedError extends Error {
  name = "MailFailedError";
}
