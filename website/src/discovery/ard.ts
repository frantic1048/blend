import { Ajv2020 } from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import schema from "./ard-entry.schema.json";

/*
 * The Agentic Resource Discovery manifest, served at /.well-known/ard.json. It
 * lists only documents this site really publishes. The manifest is validated
 * against the vendored ARD schema (ard-entry.schema.json, from
 * ards-project/ard-spec at 49eb4df6) while the site is built, so an invalid
 * manifest fails the build.
 */

const ajv = new Ajv2020({ allErrors: true, strict: false });
addFormats.default(ajv);
ajv.addSchema(schema);
const validateManifest = ajv.getSchema(`${schema.$id}#/$defs/ArdManifest`);
if (!validateManifest) {
  throw new Error("The ARD schema has no ArdManifest definition");
}

export function ardManifest(site: URL) {
  const publisher = site.hostname;
  const manifest = {
    entries: [
      {
        identifier: `urn:air:${publisher}:docs:user-guide`,
        displayName: "Blend user guide",
        type: "text/html",
        url: new URL("/guide/", site).href,
        description:
          "How to write Blend Orders in Nickel, deploy them to macOS and Linux, compare deployed files by meaning, and reconcile changes in either direction.",
        tags: ["blend", "dotfiles", "nickel", "documentation"],
        representativeQueries: [
          "How do I manage dotfiles declaratively with Nickel?",
          "How does Blend sync an edited config file back into its source?",
          "Which configuration formats can Blend render and compare?",
        ],
      },
      {
        identifier: `urn:air:${publisher}:docs:user-guide-markdown`,
        displayName: "Blend user guide (Markdown)",
        type: "text/markdown",
        url: new URL("/guide.md", site).href,
        description: "The Markdown source of the Blend user guide, identical to docs/GUIDE.md in the repository.",
        tags: ["blend", "dotfiles", "nickel", "documentation", "markdown"],
        representativeQueries: [
          "Blend dotfile manager guide in Markdown",
          "How do Blend ownership policies decide which side wins?",
        ],
      },
      {
        identifier: `urn:air:${publisher}:docs:changelog`,
        displayName: "Blend changelog",
        type: "text/html",
        url: new URL("/changelog/", site).href,
        description: "Blend release history, new features, fixes, and breaking changes, rendered from CHANGELOG.md in the repository.",
        tags: ["blend", "releases", "changelog"],
        representativeQueries: ["What changed in the latest Blend release?"],
      },
      {
        identifier: `urn:air:${publisher}:docs:changelog-markdown`,
        displayName: "Blend changelog (Markdown)",
        type: "text/markdown",
        url: new URL("/changelog.md", site).href,
        description: "The Markdown source of Blend's release history, identical to CHANGELOG.md in the repository.",
        tags: ["blend", "releases", "changelog", "markdown"],
        representativeQueries: ["Blend release notes in Markdown"],
      },
      {
        identifier: `urn:air:${publisher}:docs:llms-txt`,
        displayName: "Blend llms.txt",
        type: "text/markdown",
        url: new URL("/llms.txt", site).href,
        description: "A short Markdown index of Blend's website, guide, repository, and releases.",
        tags: ["blend", "dotfiles", "llms.txt"],
        representativeQueries: ["What is Blend and where is its documentation?", "Where can I download Blend releases?"],
      },
    ],
  };

  if (!validateManifest!(manifest)) {
    throw new Error(`Invalid ARD manifest: ${ajv.errorsText(validateManifest!.errors)}`);
  }
  return manifest;
}

export function ardResponse(site: URL | undefined) {
  if (!site) {
    throw new Error("The ARD manifest needs `site` set in astro.config.ts");
  }
  return new Response(`${JSON.stringify(ardManifest(site), null, 2)}\n`, {
    headers: { "Content-Type": "application/json; charset=utf-8" },
  });
}
