import { defineCollection } from "astro:content";
import { glob } from "astro/loaders";

// The user guide is authored once in the repository docs and rendered here.
const docs = defineCollection({
  loader: glob({ pattern: "GUIDE.md", base: "../docs" }),
});

// Release notes stay in the root changelog maintained by release automation.
const releases = defineCollection({
  loader: glob({ pattern: "CHANGELOG.md", base: ".." }),
});

export const collections = { docs, releases };
