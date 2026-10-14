import type { APIRoute } from "astro";
import { ardResponse } from "../../discovery/ard.ts";

export const GET: APIRoute = ({ site }) => ardResponse(site);
