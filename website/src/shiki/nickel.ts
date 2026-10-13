import type { LanguageRegistration } from "shiki";
import upstream from "./nickel.tmLanguage.json" with { type: "json" };

/*
 * `nickel.tmLanguage.json` is vendored verbatim from nickel-lang/nickel
 * (MIT), lsp/vscode-extension/syntaxes, commit f5f0ed52554a. Fixes for
 * constructs Blend examples rely on are applied here so upstream updates
 * remain a plain file replacement.
 */

const ident = "[A-Za-z_][\\w'-]*";

const repository = {
  ...upstream.repository,

  // Upstream only matches decimals with a fractional part.
  numbers: {
    patterns: [
      {
        name: "constant.numeric",
        match: "(?<![\\w'])-?\\d+(?:\\.\\d+)?(?:[eE][+-]?\\d+)?\\b",
      },
    ],
  },

  // Longest operators first; adds `|>`, `!=`, and `..`.
  operators: {
    patterns: [
      {
        name: "keyword.operator",
        match: "(\\|>|=>|->|==|!=|>=|<=|&&|\\|\\||\\+\\+|\\.\\.|[=|<>+\\-*/&.:@!])",
      },
    ],
  },

  // Upstream anchors tags on `\b'`, which never matches after whitespace,
  // and lets the identifier rules claim the tag name first.
  enum_tags: {
    patterns: [
      {
        name: "constant.other.enum-tag",
        match: `(?<![\\w'])'${ident}`,
      },
      {
        name: "constant.other.enum-tag",
        begin: "(?<![\\w'])'\"",
        end: '"',
        patterns: [{ name: "constant.character.escape", match: "\\\\." }],
      },
    ],
  },

  // Record field definitions: `name =`, `name | Contract =`, `name | default =`.
  fields: {
    patterns: [
      {
        match: `(?<![\\w'.])(${ident})\\s*(?=(?:\\|[^=|>]*)?=(?![=>]))`,
        captures: { 1: { name: "variable.other.property" } },
      },
    ],
  },
};

const nickel: LanguageRegistration = {
  ...upstream,
  name: "nickel",
  displayName: "Nickel",
  aliases: ["ncl"],
  patterns: [
    { include: "#comments" },
    { include: "#keywords" },
    { include: "#multiline_strings" },
    { include: "#strings" },
    { include: "#enum_tags" },
    { include: "#records" },
    { include: "#arrays" },
    { include: "#storage" },
    { include: "#fields" },
    { include: "#numbers" },
    { include: "#bools" },
    { include: "#null" },
    { include: "#operators" },
    { include: "#types" },
    { include: "#identifiers" },
  ],
  repository,
} as LanguageRegistration;

export default nickel;
