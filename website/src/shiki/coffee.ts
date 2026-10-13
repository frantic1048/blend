import type { ThemeRegistration } from "shiki";

/* Espresso-dark code theme matching the site's coffee palette. */

const palette = {
  espresso: "#2b1d16",
  foam: "#f1e4d0",
  latte: "#e2cfb4",
  steam: "#b39a86",
  grounds: "#9a8270",
  cinnamon: "#e8956f",
  honey: "#e9bb74",
  matcha: "#bcc78e",
  rose: "#e0b3a2",
  chino: "#a4c6df",
};

const coffee: ThemeRegistration = {
  name: "blend-coffee",
  type: "dark",
  colors: {
    "editor.background": palette.espresso,
    "editor.foreground": palette.latte,
  },
  settings: [
    { settings: { foreground: palette.latte, background: palette.espresso } },
    {
      scope: ["comment", "punctuation.definition.comment"],
      settings: { foreground: palette.grounds, fontStyle: "italic" },
    },
    {
      scope: ["keyword", "keyword.control", "storage", "storage.type", "storage.modifier"],
      settings: { foreground: palette.cinnamon },
    },
    {
      scope: ["keyword.operator", "punctuation"],
      settings: { foreground: palette.steam },
    },
    {
      scope: ["string", "string.quoted"],
      settings: { foreground: palette.matcha },
    },
    {
      scope: ["constant.character.escape", "meta.embedded.line", "punctuation.section.embedded"],
      settings: { foreground: palette.honey },
    },
    {
      scope: ["constant.numeric", "constant.boolean", "constant.null", "constant.language"],
      settings: { foreground: palette.honey },
    },
    {
      scope: ["constant.other.enum-tag"],
      settings: { foreground: palette.chino },
    },
    {
      scope: ["variable.uppercase-identifier", "support.class", "entity.name.type"],
      settings: { foreground: palette.rose },
    },
    {
      scope: [
        "variable.other.property",
        "support.type.property-name",
        "entity.name.tag",
        "variable.other.key",
        "meta.object-literal.key",
      ],
      settings: { foreground: palette.foam },
    },
    {
      scope: ["entity.name.function", "support.function", "entity.name.command"],
      settings: { foreground: palette.honey },
    },
    {
      scope: ["variable.parameter", "constant.other.option"],
      settings: { foreground: palette.rose },
    },
  ],
};

export default coffee;
