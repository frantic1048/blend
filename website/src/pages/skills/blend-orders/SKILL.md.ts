import type { APIRoute } from "astro";
import skill from "../../../../../skills/blend-orders/SKILL.md?raw";

// Keep the canonical skill, including its frontmatter, byte-for-byte intact.
export const prerender = true;
export const GET: APIRoute = () =>
  new Response(skill, {
    headers: { "Content-Type": "text/markdown; charset=utf-8" },
  });
