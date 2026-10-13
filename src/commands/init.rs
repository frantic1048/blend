use anyhow::{Context as AnyhowContext, bail};

use crate::commands::sync::cmd_sync;
use crate::context::Context;
use crate::nickel::managed_files;
use crate::output::log;
use crate::sync::{SyncMode, TerminalPrompter};

/// Init command: write or refresh `orders/order.contract.ncl` and
/// `orders/metadata.ncl`, then bootstrap a starter blend config order for new
/// blend directories. Honors `--dry-run` by checking freshness (read-only)
/// instead of writing.
///
/// When `upgrade` is true, breaking contract migrations are allowed.
pub fn cmd_init(ctx: &Context, upgrade: bool) -> anyhow::Result<()> {
    if ctx.dry_run {
        return managed_files::assert_orders_ready(&ctx.orders_dir);
    }

    // Source discovery uses orders/ as the checkout boundary. The blend Order
    // is optional and must not determine whether an existing Source is new.
    let should_create_starter = !ctx.orders_dir.is_dir();
    if should_create_starter {
        ensure_scaffold_target_clean(ctx)?;
    }

    managed_files::ensure_orders_ready(&ctx.orders_dir, upgrade)?;

    if should_create_starter {
        write_blend_starter(ctx)?;
        let orders = vec!["blend".to_string()];
        cmd_sync(
            ctx,
            &orders,
            SyncMode::Interactive,
            false,
            &TerminalPrompter,
        )?;
    }

    Ok(())
}

fn ensure_scaffold_target_clean(ctx: &Context) -> anyhow::Result<()> {
    if !ctx.blend_dir.exists() {
        return Ok(());
    }

    let entries = std::fs::read_dir(&ctx.blend_dir)
        .with_context(|| format!("Failed to read {}", ctx.blend_dir.display()))?;

    let mut dirty = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        match name.as_ref() {
            ".git" => {}
            "orders" => bail!(
                "{} is not a directory. `blend init` requires orders/ to be a directory.",
                ctx.orders_dir.display()
            ),
            _ => dirty.push(name.into_owned()),
        }
    }

    if !dirty.is_empty() {
        dirty.sort();
        bail!(
            "{} is not clean enough for `blend init`: found {}. Run init in an empty directory or pass --blend-dir to a clean path.",
            ctx.blend_dir.display(),
            dirty.join(", ")
        );
    }

    Ok(())
}

fn write_blend_starter(ctx: &Context) -> anyhow::Result<()> {
    let order_dir = ctx.orders_dir.join("blend");
    std::fs::create_dir_all(&order_dir)
        .with_context(|| format!("Failed to create {}", order_dir.display()))?;

    let path = order_dir.join("order.ncl");
    std::fs::write(&path, starter_ncl())
        .with_context(|| format!("Failed to write {}", path.display()))?;
    log::info(&format!("created {}", path.display()));
    Ok(())
}

fn starter_ncl() -> String {
    r#"let { Order, BlendOrder, .. } = import "../order.contract.ncl" in
{
  blend = {
    prefix = ["~/.config/blend/"],
    files = [
      {
        name = "config.toml",
        from_config = {
          sandbox = "prefer",
        },
      },
    ],
  },
} | Order | BlendOrder
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;
    use tempfile::TempDir;

    fn ctx_for(root: &std::path::Path) -> Context {
        let cli = Cli::parse_from([
            "blend",
            "--blend-dir",
            root.to_str().unwrap(),
            "--home",
            root.to_str().unwrap(),
        ]);
        let mut ctx = Context::new(&cli).unwrap();
        ctx.state = crate::state::StateStore::with_root(root.join(".state/snapshots"));
        ctx
    }

    #[test]
    fn cmd_init_creates_files() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(tmp.path());
        cmd_init(&ctx, false).unwrap();
        assert!(tmp.path().join("orders/order.contract.ncl").exists());
        assert!(tmp.path().join("orders/metadata.ncl").exists());
        assert!(tmp.path().join("orders/blend/order.ncl").exists());
        assert!(tmp.path().join(".config/blend/config.toml").exists());
    }

    #[test]
    fn cmd_init_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(tmp.path());
        cmd_init(&ctx, false).unwrap();
        cmd_init(&ctx, false).unwrap(); // second call must not error
    }

    #[test]
    fn cmd_init_refuses_dirty_new_scaffold_dir() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("README.md"), "already mine\n").unwrap();
        let ctx = ctx_for(tmp.path());

        let err = cmd_init(&ctx, false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not clean enough"), "got: {msg}");
        assert!(!tmp.path().join("orders/order.contract.ncl").exists());
    }

    #[test]
    fn cmd_init_reports_orders_file_without_writes() {
        let tmp = TempDir::new().unwrap();
        let orders_path = tmp.path().join("orders");
        std::fs::write(&orders_path, "user file\n").unwrap();
        let ctx = ctx_for(tmp.path());

        for upgrade in [false, true] {
            let err = cmd_init(&ctx, upgrade).unwrap_err();
            assert!(
                err.to_string().contains("orders is not a directory"),
                "got: {err}"
            );
            assert_eq!(
                std::fs::read_to_string(&orders_path).unwrap(),
                "user file\n"
            );
            assert!(!tmp.path().join(".config").exists());
            assert!(!tmp.path().join(".state").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn cmd_init_reports_broken_orders_symlink_without_writes() {
        let tmp = TempDir::new().unwrap();
        let orders_path = tmp.path().join("orders");
        std::os::unix::fs::symlink("missing-orders", &orders_path).unwrap();
        let ctx = ctx_for(tmp.path());

        for upgrade in [false, true] {
            let err = cmd_init(&ctx, upgrade).unwrap_err();
            assert!(
                err.to_string().contains("orders is not a directory"),
                "got: {err}"
            );
            assert_eq!(
                std::fs::read_link(&orders_path).unwrap(),
                std::path::Path::new("missing-orders")
            );
            assert!(!tmp.path().join("missing-orders").exists());
            assert!(!tmp.path().join(".config").exists());
            assert!(!tmp.path().join(".state").exists());
        }
    }

    #[test]
    fn cmd_init_refreshes_existing_source_without_blend_order() {
        let tmp = TempDir::new().unwrap();
        let order_dir = tmp.path().join("orders/example");
        std::fs::create_dir_all(&order_dir).unwrap();
        let order = "{ example = { prefix = [\"~/.config/example/\"], files = [{ from_file = \"config.txt\" }] } }\n";
        std::fs::write(order_dir.join("order.ncl"), order).unwrap();
        std::fs::write(order_dir.join("config.txt"), "user config\n").unwrap();
        std::fs::write(tmp.path().join("README.md"), "existing Source\n").unwrap();
        std::fs::write(
            tmp.path().join("orders/order.contract.ncl"),
            "{\n  contract_version = 3,\n}\n",
        )
        .unwrap();
        std::fs::write(tmp.path().join("orders/metadata.ncl"), "stale metadata\n").unwrap();

        let ctx = ctx_for(tmp.path());
        cmd_init(&ctx, false).unwrap();

        managed_files::assert_orders_ready(&ctx.orders_dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(order_dir.join("order.ncl")).unwrap(),
            order
        );
        assert_eq!(
            std::fs::read_to_string(order_dir.join("config.txt")).unwrap(),
            "user config\n"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("README.md")).unwrap(),
            "existing Source\n"
        );
        assert!(!ctx.orders_dir.join("blend").exists());
        assert!(!tmp.path().join(".config").exists());
        assert!(!tmp.path().join(".state").exists());
    }

    #[test]
    fn cmd_init_upgrades_existing_source_without_blend_order() {
        let tmp = TempDir::new().unwrap();
        let order_dir = tmp.path().join("orders/example");
        std::fs::create_dir_all(&order_dir).unwrap();
        let order = "{ example = { files = [] } }\n";
        std::fs::write(order_dir.join("order.ncl"), order).unwrap();
        std::fs::write(tmp.path().join("README.md"), "existing Source\n").unwrap();
        let contract = "{\n  contract_version = 2,\n}\n";
        let contract_path = tmp.path().join("orders/order.contract.ncl");
        std::fs::write(&contract_path, contract).unwrap();

        let ctx = ctx_for(tmp.path());
        let err = cmd_init(&ctx, false).unwrap_err();
        assert!(
            err.to_string().contains("blend init --upgrade"),
            "got: {err}"
        );
        assert_eq!(std::fs::read_to_string(&contract_path).unwrap(), contract);
        assert!(!ctx.orders_dir.join("metadata.ncl").exists());
        assert!(!ctx.orders_dir.join("blend").exists());

        cmd_init(&ctx, true).unwrap();

        managed_files::assert_orders_ready(&ctx.orders_dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(order_dir.join("order.ncl")).unwrap(),
            order
        );
        assert!(!ctx.orders_dir.join("blend").exists());
        assert!(!tmp.path().join(".config").exists());
        assert!(!tmp.path().join(".state").exists());
    }

    #[test]
    fn cmd_init_refreshes_empty_orders_dir_without_starter() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("orders")).unwrap();
        std::fs::write(tmp.path().join("README.md"), "existing Source\n").unwrap();
        let ctx = ctx_for(tmp.path());

        cmd_init(&ctx, false).unwrap();

        managed_files::assert_orders_ready(&ctx.orders_dir).unwrap();
        assert!(!ctx.orders_dir.join("blend").exists());
        assert!(!tmp.path().join(".config").exists());
        assert!(!tmp.path().join(".state").exists());
    }

    #[test]
    fn cmd_init_does_not_overwrite_existing_blend_order() {
        let tmp = TempDir::new().unwrap();
        let order_dir = tmp.path().join("orders/blend");
        std::fs::create_dir_all(&order_dir).unwrap();
        let order = order_dir.join("order.ncl");
        std::fs::write(&order, "{ blend = { files = [] } }\n").unwrap();

        let ctx = ctx_for(tmp.path());
        cmd_init(&ctx, false).unwrap();

        assert_eq!(
            std::fs::read_to_string(order).unwrap(),
            "{ blend = { files = [] } }\n"
        );
        assert!(!tmp.path().join(".config/blend/config.toml").exists());
    }
}
