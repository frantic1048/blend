import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const canonical = await readFile(new URL("../../skills/blend-orders/SKILL.md", import.meta.url));
const published = await readFile(new URL("../dist/skills/blend-orders/SKILL.md", import.meta.url));
assert.deepEqual(published, canonical, "The website must publish the canonical skill verbatim");

// The skill's example is exercised by tests/skill_e2e.rs; the homepage must show the same Order.
const skillOrder = canonical.toString("utf8").split("```nickel\n")[1].split("```")[0];
const homepageOrder = await readFile(new URL("../src/examples/author-editor.ncl", import.meta.url), "utf8");
assert.equal(homepageOrder, skillOrder, "The homepage Order example must match the skill's tested example");

const redirects = await readFile(new URL("../dist/_redirects", import.meta.url), "utf8");
assert.ok(
  redirects.split(/\r?\n/).includes(
    "/skills/:ref/blend-orders/SKILL.md https://raw.githubusercontent.com/frantic1048/blend/:ref/skills/blend-orders/SKILL.md 302",
  ),
  "The website must ship the versioned skill redirect",
);
console.log("Published skill bytes, homepage example, and versioned redirect verified.");
