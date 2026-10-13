import { defineConfig } from "astro/config";
import coffee from "./src/shiki/coffee.ts";
import nickel from "./src/shiki/nickel.ts";

export default defineConfig({
  output: "static",
  markdown: {
    shikiConfig: {
      theme: coffee,
      langs: [nickel],
    },
  },
});
