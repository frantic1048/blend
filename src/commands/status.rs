use console::style;
use rayon::prelude::*;

use crate::commands::helpers::{Assessment, SyncState, assess_result};
use crate::compose::{BuildResult, build_entry_targets, discover_orders, get_order};
use crate::context::Context;
use crate::nickel::ResourceKind;
use crate::nickel::managed_files;
use crate::output::log;

/// A styled table cell, padded at render time.
enum Cell {
    Ok(&'static str),
    Warn(&'static str),
    Error(&'static str),
    Dim(&'static str),
}

impl Cell {
    fn render(&self, width: usize) -> String {
        match self {
            Self::Ok(text) => style(format!("{text:<width$}")).green().to_string(),
            Self::Warn(text) => style(format!("{text:<width$}")).yellow().to_string(),
            Self::Error(text) => style(format!("{text:<width$}")).red().to_string(),
            Self::Dim(text) => style(format!("{text:<width$}")).dim().to_string(),
        }
    }
}

const IN_SYNC: &str = "\u{2713}";
const DIFFERS: &str = "\u{2260}";
const NONE: &str = "\u{00b7}";

/// Present a shared assessment as the STATUS and DIFF cells of one row.
fn status_cells(result: &BuildResult, assessment: &Assessment) -> (Cell, Cell) {
    let in_sync_label = if result.is_symlink {
        "linked"
    } else if result.kind == ResourceKind::Directory {
        "directory"
    } else {
        "deployed"
    };
    match assessment.state {
        SyncState::Unmanaged => (Cell::Dim("unmanaged"), Cell::Dim(NONE)),
        SyncState::Unresolved => (Cell::Warn("unresolved"), Cell::Warn(DIFFERS)),
        SyncState::Absent => (Cell::Ok("absent"), Cell::Ok(IN_SYNC)),
        SyncState::WouldRemove | SyncState::TypeMismatch { .. } => {
            (Cell::Warn("mismatch"), Cell::Warn(DIFFERS))
        }
        SyncState::NotDeployed => (Cell::Warn("pending"), Cell::Dim(NONE)),
        SyncState::NotCompared => (Cell::Ok(in_sync_label), Cell::Dim(NONE)),
        SyncState::InSync => {
            let empty_tree = assessment.file_diffs.as_ref().is_some_and(Vec::is_empty);
            let diff = if empty_tree {
                Cell::Dim(NONE)
            } else {
                Cell::Ok(IN_SYNC)
            };
            (Cell::Ok(in_sync_label), diff)
        }
        SyncState::Changed => {
            let inner_type_mismatch = assessment
                .file_diffs
                .iter()
                .flatten()
                .any(|diff| diff.has_type_mismatch());
            if result.is_symlink || result.kind == ResourceKind::Directory || inner_type_mismatch {
                (Cell::Warn("mismatch"), Cell::Warn(DIFFERS))
            } else {
                (Cell::Ok("deployed"), Cell::Warn(DIFFERS))
            }
        }
    }
}

/// Status command: show available orders and their state
pub fn cmd_status(ctx: &Context) -> anyhow::Result<()> {
    managed_files::assert_orders_ready(&ctx.orders_dir)?;

    let orders = discover_orders(&ctx.orders_dir);
    log::success(&format!("Found {} orders in orders/", orders.len()));

    let order_w = 20;
    let file_w = 20;
    let status_w = 10;
    let diff_w = 5;

    println!(
        "\n{} {} {} {} {}",
        style(format!("{:<order_w$}", "ORDER")).bold(),
        style(format!("{:<file_w$}", "FILE")).bold(),
        style(format!("{:<status_w$}", "STATUS")).bold(),
        style(format!("{:<diff_w$}", "DIFF")).bold(),
        style("TARGET").bold()
    );
    println!("{}", "-".repeat(order_w + file_w + status_w + diff_w + 40));

    let mut order_list: Vec<_> = orders.into_iter().collect::<Vec<_>>();
    order_list.sort();

    let timing = std::env::var("BLEND_TIMING").is_ok();
    let row_groups: Vec<Vec<String>> = order_list
        .par_iter()
        .map(|order_name| {
            let t_order = std::time::Instant::now();
            let mut rows = Vec::new();
            match get_order(ctx, order_name) {
                Ok(order) => {
                    let applies = order.should_apply(
                        &ctx.metadata.os,
                        &ctx.metadata.arch,
                        &ctx.metadata.hostname,
                    );

                    if !applies {
                        rows.push(format!(
                            "{} {} {} {} {}",
                            style(format!("{:<order_w$}", order_name)).dim(),
                            style(format!("{:<file_w$}", "-")).dim(),
                            style(format!("{:<status_w$}", "skipped")).dim(),
                            style(format!("{:<diff_w$}", "\u{00b7}")).dim(),
                            style("(condition not met)").dim()
                        ));
                        return rows;
                    }

                    let files = &order.blend.files;
                    let mut shown_order = false;
                    for (file_entry_index, file_entry) in files.iter().enumerate() {
                        let file_applies = file_entry.should_apply(
                            &ctx.metadata.os,
                            &ctx.metadata.arch,
                            &ctx.metadata.hostname,
                        );

                        if !file_applies {
                            if ctx.verbose {
                                let order_display = if shown_order {
                                    String::new()
                                } else {
                                    shown_order = true;
                                    order_name.to_string()
                                };
                                rows.push(format!(
                                    "{} {} {} {} {}",
                                    style(format!("{:<order_w$}", order_display)).dim(),
                                    style(format!("{:<file_w$}", file_entry.name)).dim(),
                                    style(format!("{:<status_w$}", "skipped")).dim(),
                                    style(format!("{:<diff_w$}", "\u{00b7}")).dim(),
                                    style("(condition not met)").dim()
                                ));
                            }
                            continue;
                        }

                        for (j, (target, result)) in
                            build_entry_targets(ctx, order_name, &order, file_entry_index)
                                .into_iter()
                                .enumerate()
                        {
                            let order_display = if !shown_order && j == 0 {
                                shown_order = true;
                                style(format!("{:<order_w$}", order_name))
                                    .cyan()
                                    .to_string()
                            } else {
                                format!("{:<order_w$}", "")
                            };

                            let result = result.and_then(|result| {
                                let assessment = assess_result(&result)?;
                                Ok((result, assessment))
                            });

                            let source_name = &file_entry.name;
                            let is_dir = result
                                .as_ref()
                                .is_ok_and(|(result, _)| result.source_is_directory());
                            let source_display = if source_name.len() > file_w {
                                format!("{:<file_w$}", format!("{}...", &source_name[..file_w - 3]))
                            } else if is_dir {
                                format!("{:<file_w$}", format!("{}/", source_name))
                            } else {
                                format!("{:<file_w$}", source_name)
                            };

                            let (status, diff_display, row_error) = match &result {
                                Ok((result, assessment)) => {
                                    let (status, diff) = status_cells(result, assessment);
                                    (status.render(status_w), diff.render(diff_w), None)
                                }
                                Err(error) => (
                                    Cell::Error("error").render(status_w),
                                    Cell::Dim("\u{00b7}").render(diff_w),
                                    Some(format!("{error:#}")),
                                ),
                            };

                            let target_str = target.to_string_lossy();
                            let home_str = ctx.home_dir.to_string_lossy();
                            let target_display = if target_str.starts_with(home_str.as_ref()) {
                                format!("~{}", &target_str[home_str.len()..])
                            } else {
                                target_str.into_owned()
                            };

                            let target_display = if let Some(error) = row_error {
                                format!("{target_display} ({error})")
                            } else {
                                target_display
                            };

                            rows.push(format!(
                                "{} {} {} {} {}",
                                order_display, source_display, status, diff_display, target_display
                            ));
                        }
                    }
                }
                Err(e) => {
                    let dash_display = format!("{:<file_w$}", "-");
                    rows.push(format!(
                        "{} {} {} {} {}",
                        style(format!("{:<order_w$}", order_name)).red(),
                        dash_display,
                        style(format!("{:<status_w$}", "error")).red(),
                        style(format!("{:<diff_w$}", "\u{00b7}")).dim(),
                        style(e.to_string()).red()
                    ));
                }
            }
            if timing {
                eprintln!(
                    "[timing] order {} total={}us rows={}",
                    order_name,
                    t_order.elapsed().as_micros(),
                    rows.len()
                );
            }
            rows
        })
        .collect();

    for rows in row_groups {
        for row in rows {
            println!("{}", row);
        }
    }

    println!();
    log::info(&format!(
        "System: {} / {} / {}",
        ctx.metadata.os, ctx.metadata.arch, ctx.metadata.hostname
    ));

    Ok(())
}
