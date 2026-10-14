import type { APIRoute } from "astro";
import { getEntry } from "astro:content";

// The guide's Markdown source, served verbatim for readers and tools that prefer it.
export const GET: APIRoute = async () => {
  const guide = await getEntry("docs", "guide");
  if (!guide?.body) {
    throw new Error("docs/GUIDE.md was not loaded");
  }
  return new Response(guide.body.endsWith("\n") ? guide.body : `${guide.body}\n`, {
    headers: { "Content-Type": "text/markdown; charset=utf-8" },
  });
};
