// What an approval is about, as a person reads it on a phone: the daemon's label, and the command with every character that could hide
// something made visible. Bidi marks and zero-width characters can make a command read as something else; a line of the page must not.
import { t } from "./i18n";

export interface Risk {
  level: "low" | "medium" | "dangerous" | "unknown";
  tags: string[];
}

/** What the daemon says about a wait. Its command is absent when no hook has said what would run. */
export interface WaitInfo {
  command: string | null;
  risk: Risk;
}

/** A piece of a command: as it is, or a character written out because it does not show as itself. */
export interface CommandPart {
  text: string;
  escaped: boolean;
}

/** The same characters the daemon's label counts as hiding something (`risk::hidden_mark`), with the controls. New lines and tabs stay: they lay a command out. */
const HIDDEN = /[\u0000-\u0008\u000B-\u001F\u007F-\u009F­͏؜ᅟᅠ឴឵᠋-᠏​-‏‪-‮⁠-⁯ㅤ︀-️﻿ﾠ￹-￻]|[\u{E0000}-\u{E0FFF}]/gu;

const written = (c: string) => `\\u{${(c.codePointAt(0) ?? 0).toString(16).toUpperCase().padStart(4, "0")}}`;

/** The command in pieces, hostile characters written out. Nothing is left out: what is shown is every character of it. */
export function commandParts(command: string): CommandPart[] {
  const parts: CommandPart[] = [];
  let at = 0;
  for (const m of command.matchAll(HIDDEN)) {
    if (m.index > at) parts.push({ text: command.slice(at, m.index), escaped: false });
    parts.push({ text: written(m[0]), escaped: true });
    at = m.index + m[0].length;
  }
  if (at < command.length) parts.push({ text: command.slice(at), escaped: false });
  return parts;
}

/** The command as one string, for what is not a page's markup. */
export const escapeCommand = (command: string) => commandParts(command).map((p) => p.text).join("");

/** A word for the level. An unknown level is what `unknown` means. */
export function levelText(risk: Risk | null | undefined): string {
  switch (risk?.level) {
    case "low":
      return t("Low risk");
    case "medium":
      return t("Medium risk");
    case "dangerous":
      return t("Dangerous");
    default:
      return t("Risk unknown");
  }
}

/** The level as a page may style it: nothing a computer says gets into a class but these. */
export const levelClass = (risk: Risk | null | undefined) => (risk && ["low", "medium", "dangerous"].includes(risk.level) ? risk.level : "unknown");

export function tagText(tag: string): string {
  switch (tag) {
    case "network":
      return t("uses the network");
    case "delete":
      return t("deletes files");
    case "outside_worktree":
      return t("reaches outside the project");
    case "git_push":
      return t("pushes to a remote");
    case "secret_access":
      return t("reads secrets");
    case "install":
      return t("installs software");
    case "sudo":
      return t("runs as administrator");
    case "pipe_to_shell":
      return t("runs downloaded or generated code");
    case "edits_config":
      return t("changes hooks, CI or agent settings");
    default:
      return tag;
  }
}
