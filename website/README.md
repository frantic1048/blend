# Blend website

The source for Blend's project website lives here. It is a static Astro site
with no client-side JavaScript or external styling framework.

- `/guide/` renders `../docs/GUIDE.md` directly; edit the guide there.
- Code is highlighted at build time by Shiki with a custom theme
  (`src/shiki/coffee.ts`). Nickel uses the upstream TextMate grammar vendored
  in `src/shiki/nickel.tmLanguage.json`, patched by `src/shiki/nickel.ts`.
- Fonts in `public/fonts/` are Latin subsets of the Fraunces, EB Garamond, and
  JetBrains Mono variable fonts (SIL OFL 1.1, licenses alongside). They are
  stored with Git LFS; run `git lfs pull` if they appear as pointer files.

## Development

From this directory:

```sh
pnpm install
pnpm dev
```

Before submitting changes, run:

```sh
pnpm check
pnpm build
```

The production output is written to `dist/`.

## Deployment

Website CI deploys `dist/` to Cloudflare Pages: pushes to `main` become the
production deployment and same-repository pull requests get preview
deployments. It reads the `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`
secrets and the `CLOUDFLARE_PROJECT_NAME` variable.
