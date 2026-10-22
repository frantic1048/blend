import type { APIRoute } from "astro";
import { getEntry } from "astro:content";

export const GET: APIRoute = async () => {
  const changelog = await getEntry("releases", "changelog");
  if (!changelog?.body) {
    throw new Error("CHANGELOG.md was not loaded");
  }
  return new Response(changelog.body.endsWith("\n") ? changelog.body : `${changelog.body}\n`, {
    headers: { "Content-Type": "text/markdown; charset=utf-8" },
  });
};
