---
name: blend-orders
description: Author and maintain Blend dotfile Orders in Nickel, including structured configuration, field ownership, and reconciliation with application files.
---

# Author Blend Orders

Edit `orders/<name>/order.ncl` directly for precise configuration. Nickel Orders
are the authoring interface. `blend add <order> <target>` imports an existing
Target as literal `from_file` Source; it is not a general structured editor.
`blend create <order>` scaffolds an empty Order without importing a Target.

## Find the installed interface

This skill is bundled with the binary that prints it. Prefer
`blend skill print` for that installed version over the website's current copy,
which follows the development branch. Release-specific copies are available at
`https://blend.pyonpyon.today/skills/blend-v<VERSION>/blend-orders/SKILL.md`;
the same path accepts a full Git commit in place of the release tag.
Older releases that predate this command have no bundled skill.

In a checkout, read the generated `orders/order.contract.ncl` for exact fields,
valid combinations, and helper functions. Do not replace it with a hand-written
schema. `orders/metadata.ncl` supplies editor defaults; Blend injects runtime
values for `os`, `arch`, `hostname`, `user`, `home`, and `desktop`. Use the stable
`import "../metadata.ncl"` form when those values are needed.

Run commands in the intended Source root or select it with `--blend-dir`.
Otherwise Blend searches ancestor directories and then remembered state.
`blend init` creates or refreshes the generated modules. In a fresh directory
without `orders/`, it also deploys a starter Blend configuration. Existing
`orders/` trees only get generated-module refreshes. Contract upgrade diagnostics
describe when `init --upgrade` is needed; do not blindly downgrade a newer
checkout's contract to fit an older binary.

## Choose the representation and resource

- `kind = "content"` is the default. Use `from_config` for structured Nickel
  values, conditions, and field-level ownership. Set `name` and a format, or
  let the Target filename extension select the format. Supported formats include
  TOML, JSON, JSONC, YAML, plaintext, and line-oriented formats; inspect the
  generated contract for their exact names.
- Use `from_file` for literal files or directories when exact bytes, comments,
  or application formatting matter. `from_file` and optional directory `local`
  overlay paths must stay relative to the Order directory and cannot escape it.
  Directory `exclude` patterns limit the managed Source manifest.
- `kind = "symlink"` uses exactly one of `from_file` (link to Source) or
  `link_to` (literal destination). A relative `link_to` is resolved from the
  Target's parent. Do not use the removed `symlink` boolean.
- `kind = "directory"` with `name` manages only directory existence and optional
  `mode`, without claiming its contents. It has no content input.

Every entry needs a Target `prefix` from itself or its Order. An entry prefix
replaces the Order default; multiple prefixes deploy to multiple Targets.
Optional `when` filters select OS, architecture, or hostname. Do not overlap
Targets already owned by another entry. `mode`, where supported by the contract,
enforces Unix permissions independently of content sync direction.

## Declare ownership deliberately

Ordinary literals participate in reconciliation, but a difference remains
interactive. Use `'Enforce value` to require an exact value automatically.
A resolver `fun { target } => ...` observes the current Target, or `'Absent`
when missing; returning a scalar enforces it, while a returned record recurses
through its children. Ordinary child literals still remain interactive.

- `'Absent` removes a field or resource; it is not `null`.
- `'Unmanaged` leaves the observed Target outside Blend ownership.
- `'Unresolved` explicitly leaves a resolver decision for interactive/manual
  handling; Blend does not invent edits to the resolver.
- `'Assert expected` or `'Assert (fun { target } => ...)` checks a requirement
  and fails rather than repairing it.

Use `{ ... } |> blend.with_target_only 'Unmanaged` to leave undeclared
application-owned children alone. Place the policy on each record boundary
where extra children must be retained, including nested records. Whole-record
`'Enforce { ... }` claims exact structure, so it is unsuitable for partial
ownership of that record. These enum tags are reserved resolution semantics,
not ordinary configuration values.

Structured reconciliation preserves values semantically, not the original
comments, whitespace, or key ordering. TOML requires representable non-null
values; use `'Absent` to express removal rather than relying on `null` rendering.
Choose `from_file` if byte fidelity is the requirement.

## Complete partial-ownership example

After generated modules exist, save this as `orders/editor/order.ncl` (or use
`blend create editor` first). It manages only `theme` in
`~/.config/editor/settings.toml`:

```nickel
let { Order, .. } = import "../order.contract.ncl" in
{
  blend = {
    prefix = ["~/.config/editor/"],
    files = [
      {
        name = "settings.toml",
        format = "toml",
        from_config =
          {
            theme = 'Enforce "dark",
          }
          |> blend.with_target_only 'Unmanaged,
      },
    ],
  },
} | Order
```

If the Target contains `theme = "light"` and an application-written `window`
table, the preview should change only `theme` semantically and retain `window`.
If no Target exists, it creates the declared setting; there are no existing
application fields to retain. To reconcile `theme` interactively instead, use
the ordinary literal `theme = "dark"`.

```sh
blend check editor
blend view editor
# When ready to reconcile:
blend sync editor
```

`check` validates Source; it does not prove that observed Target values will be
preserved. `view` evaluates against the actual Target and shows the proposed
content differences. Both commands are read-only. Inspect their output before
applying changes. `sync` can prompt and write Source, Targets, generated modules,
and state; declared enforcement can apply without a prompt. Select only the
Orders relevant to the request. `--dry-run` prevents writes but its output is
command-specific, and is not a substitute for reviewing `view`.

## Reverse sync boundaries

`from_file` can copy Target bytes back to Source. For `from_config`, Blend can
rewrite concrete literal leaves and supported structural changes, including
active metadata-driven branches. Computed expressions, resolver logic, and
ambiguous structures require direct Order edits. Do not promise that arbitrary
generated configuration can be reversed. `--no-rewrite` disables Nickel edits
and leaves manual merge behavior. Force flags are named by direction:
`--force-source-to-target` and `--force-target-to-source`; they are not needed
for the normal inspect-then-reconcile workflow.

For deeper examples, formats, and migration details, read the
[canonical guide](https://blend.pyonpyon.today/guide/). For exact installed
syntax, use CLI help and the checkout's generated contract.
