import type { LanguageRegistration } from "shiki";

/*
 * A small grammar for transcripts of Blend's own terminal output, so Source,
 * Target, and Base lines in homepage examples read the way they do in a
 * terminal. Scopes reuse ones the coffee theme already colours.
 */
const blendOutput: LanguageRegistration = {
  name: "blend-output",
  scopeName: "text.blend-output",
  repository: {},
  patterns: [
    {
      match: "^(\\$) (blend)\\b(.*)$",
      captures: {
        1: { name: "punctuation.definition.prompt" },
        2: { name: "entity.name.command" },
      },
    },
    {
      match: "^(\\s*)(~) (\\S+)$",
      captures: {
        2: { name: "keyword.operator" },
        3: { name: "variable.other.property" },
      },
    },
    {
      match: "^\\s*(<< Source:)",
      captures: { 1: { name: "variable.parameter" } },
    },
    {
      match: "^\\s*(>> Target:)",
      captures: { 1: { name: "constant.other.enum-tag" } },
    },
    { match: "^\\s*\\|\\| Base:.*$", name: "comment.line" },
    { match: "\\[snapshot\\]", name: "comment.line" },
    { match: "^Cannot apply .*$", name: "keyword.control" },
    { match: "\"[^\"]*\"", name: "string.quoted.double" },
    { match: "(?<=: )-?\\d+(\\.\\d+)?\\b", name: "constant.numeric" },
  ],
};

export default blendOutput;
