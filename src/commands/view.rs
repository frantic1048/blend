use console::style;

use crate::commands::helpers::{Assessment, SyncState, assess_presence, assess_result};
use crate::compose::{BuildResult, build_entry_targets, discover_orders, get_order};
use crate::context::Context;
use crate::fs_node::NodeKind;
use crate::nickel::managed_files;
use crate::output::log;

struct ViewOptions {
    show_content: bool,
    show_diff: bool,
    short: bool,
    viewing_specific: bool,
}

fn type_mismatch(expected: NodeKind, actual: NodeKind) -> String {
    format!("(type mismatch: expected {expected}, found {actual})")
}

fn print_with(header: &str, annotations: &[String]) {
    if annotations.is_empty() {
        println!("{header}");
    } else {
        println!("{header} {}", annotations.join(" "));
    }
}

/// Print one resource. Whether it is in sync comes from the shared
/// assessment; this only decides how much of it to show.
fn render_result(result: &BuildResult, assessment: &Assessment, header: &str, o: &ViewOptions) {
    let quiet = o.short && !o.viewing_specific;
    match assessment.state {
        SyncState::Unmanaged => {
            if !quiet {
                println!("{header} {}", style("(unmanaged)").dim());
            }
            return;
        }
        SyncState::Unresolved => {
            println!(
                "{header} {}",
                style("(unresolved; edit order.ncl manually)").yellow()
            );
            return;
        }
        SyncState::Absent => {
            if !quiet {
                println!("{header} {}", style("(absent)").green());
            }
            return;
        }
        SyncState::WouldRemove => {
            println!("{header} {}", style("(would remove target)").yellow());
            return;
        }
        _ => {}
    }

    if result.kind == crate::nickel::ResourceKind::Directory {
        let note = match assessment.state {
            SyncState::NotDeployed => "(not deployed)".to_string(),
            SyncState::TypeMismatch { expected, actual } => type_mismatch(expected, actual),
            SyncState::InSync if o.short => return,
            SyncState::InSync => "(in sync)".to_string(),
            _ => assessment.diff.output.clone(),
        };
        println!("{header} (directory) {note}");
        return;
    }

    if result.is_symlink {
        let Some(canonical) = &result.canonical_source else {
            return;
        };
        let link_status = match assessment.state {
            SyncState::InSync if o.short => return,
            SyncState::InSync => style("(linked)").green().to_string(),
            SyncState::TypeMismatch { actual, .. } => {
                style(format!("(type mismatch: expected symlink, found {actual})"))
                    .yellow()
                    .to_string()
            }
            SyncState::NotDeployed => style("(not linked)").yellow().to_string(),
            _ => style("(wrong target)").yellow().to_string(),
        };
        println!(
            "{header} {} {link_status} -> {}",
            style("(symlink)").dim(),
            canonical.display()
        );
        return;
    }

    if result.is_plaintext && result.source_path.is_some() {
        render_plaintext(result, assessment, header, o);
    } else {
        render_structured(result, assessment, header, o);
    }
}

fn render_plaintext(result: &BuildResult, assessment: &Assessment, header: &str, o: &ViewOptions) {
    let is_dir = result.source_is_directory();
    let mut annotations = Vec::new();
    if o.show_content {
        let kind = if is_dir { "directory" } else { "file" };
        annotations.push(style(format!("(plaintext {kind})")).dim().to_string());
    }
    if !o.show_diff {
        print_with(header, &annotations);
        return;
    }

    match (assessment.state, &assessment.file_diffs) {
        (SyncState::NotDeployed, _) => {
            annotations.push(style("(not deployed)").yellow().to_string());
            print_with(header, &annotations);
        }
        (SyncState::TypeMismatch { expected, actual }, _) => {
            annotations.push(style(type_mismatch(expected, actual)).yellow().to_string());
            print_with(header, &annotations);
        }
        (_, Some(file_diffs)) if file_diffs.is_empty() => {
            if !o.short {
                annotations.push(style("(empty directory)").dim().to_string());
                print_with(header, &annotations);
            }
        }
        (state, Some(file_diffs)) => {
            if state == SyncState::Changed || !o.short {
                print_with(header, &annotations);
            }
            for f in file_diffs {
                let rel = f.rel_path.display();
                if f.source_only {
                    println!(
                        "    {} {}",
                        style("+").green(),
                        style(format!("{rel} (not deployed)")).green()
                    );
                } else if f.has_changes {
                    let label = match f.target_kind {
                        Some(target_kind) if target_kind != f.expected_kind => format!(
                            "{rel} (type mismatch: expected {}, found {target_kind})",
                            f.expected_kind
                        ),
                        _ => rel.to_string(),
                    };
                    println!(
                        "    {} {}",
                        style("\u{2260}").yellow(),
                        style(label).yellow()
                    );
                    for line in f.diff_output.lines() {
                        println!("      {line}");
                    }
                } else if !o.short {
                    println!("    {}", style(format!("\u{2713} {rel}")).dim());
                }
            }
        }
        (SyncState::Changed, None) => {
            print_with(header, &annotations);
            for line in assessment.diff.output.lines() {
                println!("    {line}");
            }
        }
        (_, None) => {
            if !o.short {
                annotations.push(style("(no changes)").dim().to_string());
                print_with(header, &annotations);
            }
        }
    }
}

fn render_structured(result: &BuildResult, assessment: &Assessment, header: &str, o: &ViewOptions) {
    let note = match assessment.state {
        _ if !o.show_diff => String::new(),
        SyncState::NotDeployed => style("(not deployed)").yellow().to_string(),
        SyncState::TypeMismatch { expected, actual } => {
            style(type_mismatch(expected, actual)).yellow().to_string()
        }
        SyncState::Changed => String::new(),
        _ if o.show_content => String::new(),
        _ if o.short => return,
        _ => style("(no changes)").dim().to_string(),
    };
    if note.is_empty() {
        println!("{header}");
    } else {
        println!("{header} {note}");
    }
    if o.show_content {
        for line in result.content.lines() {
            println!("    {}", style(line).dim());
        }
    }
    if o.show_diff && assessment.state == SyncState::Changed {
        for line in assessment.diff.output.lines() {
            println!("    {line}");
        }
    }
}

/// View command: show generated config and/or diff from deployed
pub fn cmd_view(
    ctx: &Context,
    orders: &[String],
    content_only: bool,
    show_all: bool,
    short: bool,
) -> anyhow::Result<()> {
    managed_files::assert_orders_ready(&ctx.orders_dir)?;

    let all_orders = discover_orders(&ctx.orders_dir);
    let viewing_specific = !orders.is_empty();

    let to_view: Vec<String> = if orders.is_empty() {
        all_orders.into_iter().collect()
    } else {
        orders.to_vec()
    };

    let show_content = content_only || show_all;
    let show_diff = !content_only;
    let options = ViewOptions {
        show_content,
        show_diff,
        short,
        viewing_specific,
    };
    let mut has_changes = false;
    let mut orders_found = 0;
    let mut evaluation_errors = 0;
    let mut comparison_errors = 0;

    let shorten_path = |path: &std::path::Path| -> String {
        let s = path.to_string_lossy();
        let home = ctx.home_dir.to_string_lossy();
        if s.starts_with(home.as_ref()) {
            format!("~{}", &s[home.len()..])
        } else {
            s.into_owned()
        }
    };

    for order_name in &to_view {
        if !ctx.orders_dir.join(order_name).join("order.ncl").exists() {
            log::error(&format!("Order '{order_name}' not found"));
            continue;
        }
        orders_found += 1;
        let order = match get_order(ctx, order_name) {
            Ok(order) => order,
            Err(e) => {
                log::error(&format!("Failed to evaluate {order_name}: {e}"));
                evaluation_errors += 1;
                continue;
            }
        };
        let (os, arch, host) = (&ctx.metadata.os, &ctx.metadata.arch, &ctx.metadata.hostname);
        let builds: Vec<_> = if order.should_apply(os, arch, host) {
            (0..order.blend.files.len())
                .filter(|&index| order.blend.files[index].should_apply(os, arch, host))
                .flat_map(|index| {
                    let name = &order.blend.files[index].name;
                    build_entry_targets(ctx, order_name, &order, index)
                        .into_iter()
                        .map(move |(target, result)| (name, target, result))
                })
                .collect()
        } else {
            Vec::new()
        };
        if builds.is_empty() {
            if viewing_specific {
                log::info(&format!("{order_name} skipped (condition not met)"));
            }
            continue;
        }

        println!("\n{}", style(order_name).cyan().bold());

        for (name, target, result) in &builds {
            let result = match result {
                Ok(result) => result,
                Err(error) => {
                    // Like a status error row: mark it and show the rest. The
                    // diagnostic (often a multi-line Nickel report) goes to stderr.
                    println!(
                        "  {name} -> {} {}",
                        shorten_path(target),
                        style("(error: evaluation failed)").red()
                    );
                    log::error(&format!("Failed to evaluate {order_name}:{name}: {error}"));
                    evaluation_errors += 1;
                    continue;
                }
            };
            // Content-only output never shows a diff, so do not read Targets for one.
            let assessment = if show_diff {
                assess_result(result)
            } else {
                assess_presence(result)
            };
            let immutable_tag = if result.immutable {
                format!(" {}", style("(immutable)").magenta())
            } else {
                String::new()
            };
            let file_header = format!(
                "  {name} -> {}{immutable_tag}",
                shorten_path(&result.target)
            );
            match assessment {
                Ok(assessment) => {
                    if show_diff {
                        has_changes |= assessment.state.is_drift();
                    }
                    render_result(result, &assessment, &file_header, &options);
                }
                Err(error) => {
                    println!(
                        "{file_header} {}",
                        style(format!("(error: {error:#})")).red()
                    );
                    comparison_errors += 1;
                }
            }
        }
    }

    // Fail after rendering everything, so one bad resource hides nothing.
    let mut failures = Vec::new();
    if evaluation_errors > 0 {
        failures.push(format!("{evaluation_errors} evaluation error(s)"));
    }
    if comparison_errors > 0 {
        failures.push(format!("{comparison_errors} comparison error(s)"));
    }
    if !failures.is_empty() {
        anyhow::bail!("view failed with {}", failures.join(" and "));
    }

    if show_diff && !has_changes && orders_found > 0 {
        println!();
        log::success("All orders are up to date");
    }

    Ok(())
}
