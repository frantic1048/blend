import { defineCollection } from "astro:content";
import { glob } from "astro/loaders";

// The user guide is authored once in the repository docs and rendered here.
const docs = defineCollection({
  loader: glob({ pattern: "GUIDE.md", base: "../docs" }),
});

export const collections = { docs };
