use crate::commands::helpers::{compute_content_diff_for_result, result_has_type_mismatch};
use crate::compose::{
    self, BuildResult, build_glob_set, collect_merged_files, discover_orders, mode_mismatches,
    reconcile_file_mode, write_result,
};
use crate::context::Context;
use crate::diff::{diff_configs_with_base, key_change_with_base_display, semantic_diff_keys};
use crate::formats::get_renderer;
use crate::fs_node::node_kind;
use crate::nickel;
use crate::nickel::key_path::KeyPath;
use crate::nickel::managed_files;
use crate::nickel::resolution::ResourceDisposition;
use crate::output::log;
use crate::sync::{self, KeyAction, KeyResolution, Prompter, SyncAction, SyncMode};

/// Sync command: bidirectional sync between repo and deployed configs
pub fn cmd_sync(
    ctx: &Context,
    orders: &[String],
    mode: SyncMode,
    no_rewrite: bool,
    prompter: &dyn Prompter,
) -> anyhow::Result<()> {
    let _interrupt_guard = crate::interrupt::Guard::new()?;
    if ctx.dry_run {
        managed_files::assert_orders_ready(&ctx.orders_dir)?;
    } else {
        managed_files::ensure_orders_ready(&ctx.orders_dir, false)?;
    }

    let mut ownership = crate::resources::validate_ownership(ctx)?;

    let all_orders = discover_orders(&ctx.orders_dir);

    if all_orders.is_empty() {
        log::warn("No orders found in orders directory");
        return Ok(());
    }

    let to_sync: Vec<String> = if orders.is_empty() {
        all_orders.into_iter().collect()
    } else {
        orders
            .iter()
            .filter(|p| {
                if all_orders.contains(*p) {
                    true
                } else {
                    log::warn(&format!("Order '{p}' not found"));
                    false
                }
            })
            .cloned()
            .collect()
    };

    let mut directory_access =
        crate::resources::prepare_directory_access(ctx, &mut ownership, &to_sync)?;

    // Phase 1: build all orders and collect results
    // Also track file_entry_index per result for from_config pull-back
    let mut built: Vec<(String, Vec<(BuildResult, usize)>)> = Vec::new();
    let mut build_errors = 0;

    for order_name in &to_sync {
        if crate::interrupt::interrupted() {
            break;
        }
        match build_order_with_indices(ctx, order_name, &ownership) {
            Ok(results) => {
                if !results.is_empty() {
                    built.push((order_name.clone(), results));
                }
            }
            Err(e) => {
                log::error(&format!("Failed to build {order_name}: {e}"));
                build_errors += 1;
            }
        }
    }

    // Phase 2: for each built file, compute diff and determine action
    let mut source_to_target = 0;
    let mut target_to_source = 0;
    let mut skipped = 0;
    let mut deferred_modes = Vec::new();
    let mut directory_modes = Vec::new();

    let mut planned_directories = std::collections::HashSet::new();
    let work: Vec<_> = built
        .iter()
        .flat_map(|(order, results)| {
            results
                .iter()
                .map(move |(result, index)| (order, result, index))
        })
        .collect();
    // Preserve Order/entry order except where a selected directory must precede
    // a descendant. A global depth sort needlessly interleaves unrelated Orders.
    // Normalize each path once and compute strict-ancestor dependencies once.
    // The minimum original index among ready entries preserves prompt order.
    let paths: Vec<_> = work
        .iter()
        .map(|(_, result, _)| crate::resources::lexical_path(&result.target))
        .collect();
    let mut dependencies = vec![0usize; work.len()];
    let mut descendants = vec![Vec::new(); work.len()];
    for (child, target) in paths.iter().enumerate() {
        crate::interrupt::check()?;
        for (parent, ancestor) in paths.iter().enumerate() {
            if target != ancestor && target.starts_with(ancestor) {
                dependencies[child] += 1;
                descendants[parent].push(child);
            }
        }
    }
    let mut ready: std::collections::BinaryHeap<_> = dependencies
        .iter()
        .enumerate()
        .filter(|(_, count)| **count == 0)
        .map(|(index, _)| std::cmp::Reverse(index))
        .collect();
    let mut ordered = Vec::with_capacity(work.len());
    while let Some(std::cmp::Reverse(index)) = ready.pop() {
        ordered.push(work[index]);
        for &child in &descendants[index] {
            dependencies[child] -= 1;
            if dependencies[child] == 0 {
                ready.push(std::cmp::Reverse(child));
            }
        }
    }
    debug_assert_eq!(
        ordered.len(),
        work.len(),
        "strict ancestor dependencies are acyclic"
    );
    'resources: for (order_name, result, file_entry_index) in ordered {
        if crate::interrupt::interrupted() {
            break;
        }
        // New target file — always apply Source -> Target.
        let target_kind = match node_kind(&result.target) {
            Ok(kind) => kind,
            Err(error) => {
                log::error(&format!(
                    "Failed to inspect target for {}:{}: {}",
                    order_name, result.name, error
                ));
                build_errors += 1;
                continue;
            }
        };
        match result.resource_disposition {
            ResourceDisposition::Unmanaged => {
                if ctx.verbose {
                    log::info(&format!(
                        "{}:{} is unmanaged at the resource root",
                        order_name, result.name
                    ));
                }
                continue;
            }
            ResourceDisposition::Unresolved => {
                log::warn(&format!(
                    "Resolver made no decision for {}:{} at the resource root; edit order.ncl manually",
                    order_name, result.name
                ));
                skipped += 1;
                continue;
            }
            ResourceDisposition::Absent => {
                if target_kind.is_none() {
                    if !ctx.dry_run {
                        invalidate_content_base(ctx, order_name, result, &mut build_errors);
                    }
                    if ctx.verbose {
                        log::info(&format!(
                            "{}:{} is absent as required",
                            order_name, result.name
                        ));
                    }
                    continue;
                }
                match write_result(result, ctx.dry_run) {
                    Ok(()) => {
                        if !ctx.dry_run {
                            log::success(&format!(
                                "Applied Source -> Target absence for {}:{}",
                                order_name, result.name
                            ));
                            source_to_target += 1;
                            invalidate_content_base(ctx, order_name, result, &mut build_errors);
                        }
                    }
                    Err(error) => {
                        log::error(&format!(
                            "Failed to remove Target for {}:{}: {}",
                            order_name, result.name, error
                        ));
                        build_errors += 1;
                    }
                }
                continue;
            }
            ResourceDisposition::Present => {}
        }
        if let Err(error) = crate::resources::assert_reconciliation_ancestors(
            &ownership,
            &result.target,
            &planned_directories,
        ) {
            build_errors += 1;
            log::error(&format!("{order_name}:{}: {error:#}", result.name));
            continue;
        }
        if target_kind.is_none() && !result.manual_resolution_paths.is_empty() {
            log::warn(&format!(
                "Cannot create {}:{} because a resolver made no decision; edit order.ncl manually",
                order_name, result.name
            ));
            skipped += 1;
            continue;
        }
        if target_kind.is_none() {
            let snapshot = ctx
                .state
                .read(order_name, &result.target)
                .unwrap_or_else(|e| {
                    log::warn(&format!(
                        "snapshot read failed for {}:{}: {}",
                        order_name, result.name, e
                    ));
                    None
                });
            if let Some(annotation) = sync::file_annotation_for_deleted_target(snapshot.as_deref())
            {
                log::info_important(&format!(
                    "  [snapshot] since last sync: {}",
                    annotation.message()
                ));
            }
            if ctx.dry_run {
                crate::resources::record_planned_directories(result, &mut planned_directories);
                log::info_important(&format!(
                    "[dry-run] {}:{} -> {} (new target, would apply Source -> Target)",
                    order_name,
                    result.name,
                    result.target.display()
                ));
            } else {
                let mut deployment = result.clone();
                if deployment.kind == nickel::ResourceKind::Directory {
                    deployment.mode = None;
                }
                match write_result(&deployment, false) {
                    Ok(()) => {
                        if result.kind == nickel::ResourceKind::Directory {
                            directory_modes.push((result, true));
                        }
                        log::success(&format!(
                            "Applied Source -> Target for {}:{} -> {} (new)",
                            order_name,
                            result.name,
                            result.target.display()
                        ));
                        source_to_target += 1;
                        refresh_snapshot_for_result(ctx, order_name, result, &mut build_errors);
                    }
                    Err(e) => {
                        log::error(&format!(
                            "Failed to apply Source -> Target for {}:{}: {}",
                            order_name, result.name, e
                        ));
                        build_errors += 1;
                    }
                }
            }
            continue;
        }

        if result.kind != nickel::ResourceKind::Content
            || target_kind != Some(crate::commands::helpers::expected_node_kind(result))
        {
            let action = crate::resources::sync_resource(
                ctx,
                order_name,
                *file_entry_index,
                result,
                mode,
                no_rewrite,
                prompter,
            );
            if ctx.dry_run && matches!(action, Ok(SyncAction::ApplySourceToTarget)) {
                crate::resources::record_planned_directories(result, &mut planned_directories);
            }
            if action.is_ok() && result.kind == nickel::ResourceKind::Directory {
                match node_kind(&result.target) {
                    Ok(Some(crate::fs_node::NodeKind::Directory)) => directory_modes.push((
                        result,
                        matches!(action, Ok(SyncAction::ApplySourceToTarget)),
                    )),
                    Ok(_) => {}
                    Err(error) => {
                        build_errors += 1;
                        log::error(&format!(
                            "Failed to inspect {order_name}:{} after reconciliation: {error:#}",
                            result.name
                        ));
                    }
                }
            }
            match action {
                Ok(SyncAction::ApplySourceToTarget) if !ctx.dry_run => {
                    source_to_target += 1;
                    log::success(&format!(
                        "Applied Source -> Target for {}:{}",
                        order_name, result.name
                    ));
                }
                Ok(SyncAction::ApplyTargetToSource) if !ctx.dry_run => {
                    target_to_source += 1;
                    log::success(&format!(
                        "Applied Target -> Source for {}:{}",
                        order_name, result.name
                    ));
                }
                Ok(SyncAction::Quit) => {
                    log::info("Sync aborted by user");
                    break 'resources;
                }
                Ok(SyncAction::Skip) if result.kind != nickel::ResourceKind::Directory => {
                    match crate::commands::helpers::compute_diff_for_result(result) {
                        Ok(diff) => skipped += usize::from(diff.has_changes),
                        Err(error) => {
                            build_errors += 1;
                            log::error(&format!("{order_name}:{}: {error:#}", result.name));
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    build_errors += 1;
                    log::error(&format!("{}:{}: {error:#}", order_name, result.name));
                }
            }
            continue;
        }

        // Repair readable declared modes before comparing content. Modes
        // that remove owner read access wait until all content operations
        // (including Target -> Source reads) have completed.
        let defer_mode = result.mode.is_some_and(|mode| mode & 0o400 == 0);
        let mode_reconciled = match if defer_mode {
            mode_mismatches(result).map(|mismatches| !mismatches.is_empty())
        } else {
            reconcile_file_mode(result, ctx.dry_run)
        } {
            Ok(changed) => {
                if changed && defer_mode {
                    deferred_modes.push((order_name.as_str(), result));
                } else if changed && !ctx.dry_run {
                    log::success(&format!(
                        "Applied declared file mode for {}:{}",
                        order_name, result.name
                    ));
                    source_to_target += 1;
                }
                changed
            }
            Err(error) => {
                log::error(&format!(
                    "Failed to reconcile file mode for {}:{}: {}",
                    order_name, result.name, error
                ));
                build_errors += 1;
                continue;
            }
        };

        let diff_result = match compute_content_diff_for_result(result) {
            Ok(diff) => diff,
            Err(error) => {
                log::error(&format!(
                    "Failed to compare target for {}:{}: {}",
                    order_name, result.name, error
                ));
                build_errors += 1;
                continue;
            }
        };

        let has_type_mismatch = match result_has_type_mismatch(result) {
            Ok(mismatch) => mismatch,
            Err(error) => {
                log::error(&format!(
                    "Failed to compare target for {}:{}: {}",
                    order_name, result.name, error
                ));
                build_errors += 1;
                continue;
            }
        };
        let structural_mismatch = result.is_symlink || has_type_mismatch;

        if !diff_result.has_changes {
            if !ctx.dry_run {
                refresh_snapshot_for_result(ctx, order_name, result, &mut build_errors);
            }
            if ctx.verbose {
                log::info(&format!("{}:{} in sync", order_name, result.name));
            }
            continue;
        }

        // Determine if Target -> Source is possible for this entry.
        let can_pull = !structural_mismatch
            && !no_rewrite
            && can_auto_pull(ctx, order_name, result, *file_entry_index);

        // For from_config entries in interactive mode, use per-key flow
        let is_from_config = !result.is_plaintext && !result.is_symlink;
        // Malformed Target data cannot participate in a semantic merge, but
        // whole-file interactive mode can still repair it from Source.
        let target_is_parseable = std::fs::read_to_string(&result.target)
            .ok()
            .is_some_and(|content| get_renderer(result.format).parse(&content).is_ok());
        if is_from_config && !result.manual_resolution_paths.is_empty() && !target_is_parseable {
            log::warn(&format!(
                "Cannot resolve {}:{} automatically because its Target is not parseable and a resolver requires manual declaration editing",
                order_name, result.name
            ));
            skipped += 1;
            continue;
        }
        let use_per_key = is_from_config
            && ((matches!(mode, SyncMode::Interactive) && can_pull)
                || !result.automatic_source_paths.is_empty()
                || !result.manual_resolution_paths.is_empty())
            && target_is_parseable;

        if use_per_key {
            // Per-key interactive sync for from_config entries
            let format = result.format;
            let format_renderer = get_renderer(format);
            let deployed_content = match std::fs::read_to_string(&result.target) {
                Ok(content) => content,
                Err(error) => {
                    log::error(&format!(
                        "Could not read Target for semantic merge of {}:{}: {}",
                        order_name, result.name, error
                    ));
                    build_errors += 1;
                    continue;
                }
            };
            let source_json = match format_renderer.parse(&result.content) {
                Ok(value) => value,
                Err(error) => {
                    log::error(&format!(
                        "Could not parse generated {:?} for semantic merge of {}:{}: {}",
                        format, order_name, result.name, error
                    ));
                    build_errors += 1;
                    continue;
                }
            };
            let target_json = match format_renderer.parse(&deployed_content) {
                Ok(value) => value,
                Err(error) => {
                    log::error(&format!(
                        "Could not parse Target {:?} for semantic merge of {}:{}: {}",
                        format, order_name, result.name, error
                    ));
                    build_errors += 1;
                    continue;
                }
            };
            let key_changes = match semantic_diff_keys(
                format,
                &result.content,
                &deployed_content,
                &result.ignore_keys,
            ) {
                Ok(changes) => changes,
                Err(error) => {
                    log::error(&format!(
                        "Could not compute semantic merge for {}:{}: {}",
                        order_name, result.name, error
                    ));
                    build_errors += 1;
                    continue;
                }
            };

            if key_changes.is_empty() {
                if ctx.verbose {
                    log::info(&format!("{}:{} in sync (per-key)", order_name, result.name));
                }
                continue;
            }

            // Read & parse the snapshot once per entry so per-key annotations
            // reuse the parsed JSON. Missing or unparseable snapshots degrade
            // to unannotated 2-way (annotation = None for every key).
            let snapshot_json: Option<serde_json::Value> = match ctx
                .state
                .read(order_name, &result.target)
            {
                Ok(Some(bytes)) => {
                    let format_renderer = get_renderer(format);
                    match std::str::from_utf8(&bytes)
                        .ok()
                        .and_then(|s| format_renderer.parse(s).ok())
                    {
                        Some(v) => Some(v),
                        None => {
                            log::warn(&format!(
                                "snapshot for {}:{} is unparseable in format {:?}; falling back to 2-way",
                                order_name, result.name, format
                            ));
                            None
                        }
                    }
                }
                Ok(None) => None,
                Err(e) => {
                    log::warn(&format!(
                        "snapshot read failed for {}:{}: {}",
                        order_name, result.name, e
                    ));
                    None
                }
            };
            // Display file header
            let target_display = sync::shorten_path(&result.target, &ctx.home_dir);
            println!(
                "\n  {}:{} ({})",
                console::style(order_name).cyan(),
                result.name,
                target_display
            );

            let mut decisions: std::collections::HashMap<KeyPath, KeyResolution> =
                std::collections::HashMap::new();
            let mut all_mode: Option<KeyResolution> = None;
            let mut quit = false;

            for change in &key_changes {
                let manual_overlap = result.manual_resolution_paths.iter().any(|path| {
                    change.path.segments().starts_with(path.segments())
                        || path.segments().starts_with(change.path.segments())
                });
                if manual_overlap {
                    log::warn(&format!(
                        "Resolver made no decision for {}:{} key {}; edit order.ncl manually",
                        order_name, result.name, change.path
                    ));
                    continue;
                }
                let declarative_overlap = result.automatic_source_paths.iter().any(|path| {
                    change.path.segments().starts_with(path.segments())
                        || path.segments().starts_with(change.path.segments())
                });
                if declarative_overlap {
                    // Type mismatches are reported at the ancestor. Promote
                    // the decision so enforced descendants can be created.
                    decisions.insert(change.path.clone(), KeyResolution::Source);
                    continue;
                }
                let action = match mode {
                    SyncMode::ApplySourceToTargetAll => KeyAction::UseSource,
                    SyncMode::ApplyTargetToSourceAll => KeyAction::UseTarget,
                    SyncMode::Interactive => {
                        if ctx.dry_run {
                            log::info_important(&format!(
                                "[dry-run] would prompt for {}:{} key {}",
                                order_name, result.name, change.path
                            ));
                            KeyAction::Skip
                        } else if let Some(resolution) = all_mode {
                            match resolution {
                                KeyResolution::Source => KeyAction::UseSource,
                                KeyResolution::Target => KeyAction::UseTarget,
                            }
                        } else {
                            let key_annotation = sync::compute_key_annotation(
                                snapshot_json.as_ref(),
                                &source_json,
                                &target_json,
                                &change.path,
                            );
                            let prompt_change =
                                key_change_with_base_display(change, snapshot_json.as_ref());
                            prompter.ask_key_action(
                                order_name,
                                &result.name,
                                &prompt_change,
                                key_annotation,
                            )
                        }
                    }
                };

                match action {
                    KeyAction::UseSource => {
                        decisions.insert(change.path.clone(), KeyResolution::Source);
                    }
                    KeyAction::UseTarget => {
                        decisions.insert(change.path.clone(), KeyResolution::Target);
                    }
                    KeyAction::Skip => {
                        // No decision for this key -- skip means keep as-is
                    }
                    KeyAction::AllSource => {
                        all_mode = Some(KeyResolution::Source);
                        decisions.insert(change.path.clone(), KeyResolution::Source);
                    }
                    KeyAction::AllTarget => {
                        all_mode = Some(KeyResolution::Target);
                        decisions.insert(change.path.clone(), KeyResolution::Target);
                    }
                    KeyAction::Quit => {
                        quit = true;
                        break;
                    }
                }
            }

            if quit {
                log::info("Sync aborted by user");
                break 'resources;
            }

            if decisions.is_empty() {
                skipped += 1;
                continue;
            }

            // Apply Target -> Source FIRST: a Target-resolved key that
            // cannot be rewritten into order.ncl must not be baked into
            // the merged Target or the snapshot, otherwise the unresolved
            // conflict is silently masked. Keys that fail here are dropped
            // from `decisions` (reported as skipped) so the Target keeps
            // its deployed value and the conflict resurfaces next sync.
            let target_keys: Vec<KeyPath> = decisions
                .iter()
                .filter(|(_, resolution)| **resolution == KeyResolution::Target)
                .map(|(k, _)| k.clone())
                .collect();

            if !target_keys.is_empty() {
                if !can_pull {
                    for key in &target_keys {
                        decisions.remove(key);
                    }
                    log::warn(&format!(
                        "Skipped {} Target -> Source decision(s) for {}:{} because source rewriting is unavailable",
                        target_keys.len(),
                        order_name,
                        result.name
                    ));
                } else {
                    match sync::pull_from_config_keys(
                        ctx,
                        order_name,
                        *file_entry_index,
                        &result.target,
                        format,
                        &target_keys,
                        ctx.dry_run,
                    ) {
                        Ok(report) => {
                            for key in &report.unapplied {
                                log::warn(&format!(
                                    "Skipped {}:{} key {}: Target value could not be written back to order.ncl (resolve manually)",
                                    order_name, result.name, key
                                ));
                                decisions.remove(key);
                            }
                            if report.any_applied() {
                                target_to_source += 1;
                            }
                        }
                        Err(e) => {
                            log::error(&format!(
                                "Failed to rewrite .ncl for {}:{}: {}",
                                order_name, result.name, e
                            ));
                            build_errors += 1;
                            // None of the Target selections landed in Source;
                            // drop them so they are not treated as resolved.
                            for key in &target_keys {
                                decisions.remove(key);
                            }
                        }
                    }
                }
            }

            if decisions.is_empty() {
                // Every decision was dropped by write-back failures.
                skipped += 1;
                continue;
            }

            // Build merged JSON from the decisions that actually applied
            let merged = sync::build_merged_json(&source_json, &target_json, &decisions);

            // Write merged result to target
            match format_renderer.render(&merged) {
                Ok(merged_content) => {
                    if ctx.dry_run {
                        log::info_important(&format!(
                            "[dry-run] would apply {} declarative/selected key resolution(s) for {}:{}",
                            decisions.len(),
                            order_name,
                            result.name
                        ));
                        continue;
                    }
                    let merged_result = BuildResult {
                        validated_source: None,
                        kind: crate::nickel::ResourceKind::Content,
                        target: result.target.clone(),
                        content: merged_content,
                        is_plaintext: false,
                        source_path: None,
                        name: result.name.clone(),
                        format,
                        ignore_keys: result.ignore_keys.clone(),
                        is_symlink: false,
                        canonical_source: None,
                        exclude_patterns: vec![],
                        local_dir: None,
                        immutable: result.immutable,
                        mode: result.mode,
                        automatic_source_paths: std::collections::HashSet::new(),
                        manual_resolution_paths: std::collections::HashSet::new(),
                        resource_disposition: ResourceDisposition::Present,
                    };
                    match write_result(&merged_result, false) {
                        Ok(()) => {
                            if !mode_reconciled {
                                source_to_target += 1;
                            }
                            refresh_snapshot_for_result(
                                ctx,
                                order_name,
                                &merged_result,
                                &mut build_errors,
                            );
                            log::success(&format!(
                                "Synced {}:{} ({} keys resolved)",
                                order_name,
                                result.name,
                                decisions.len()
                            ));
                        }
                        Err(e) => {
                            log::error(&format!(
                                "Failed to write merged {}:{}: {}",
                                order_name, result.name, e
                            ));
                            build_errors += 1;
                        }
                    }
                }
                Err(e) => {
                    log::error(&format!(
                        "Failed to render merged {}:{}: {}",
                        order_name, result.name, e
                    ));
                    build_errors += 1;
                }
            }
        } else {
            // Original whole-file flow for from_file, symlinks, and
            // non-interactive / non-pullable from_config entries

            let snapshot = ctx
                .state
                .read(order_name, &result.target)
                .unwrap_or_else(|e| {
                    log::warn(&format!(
                        "snapshot read failed for {}:{}: {}",
                        order_name, result.name, e
                    ));
                    None
                });
            let diff_with_base =
                compute_diff_with_base_for_result(result, snapshot.as_deref(), structural_mismatch);
            let prompt_diff = diff_with_base.as_ref().unwrap_or(&diff_result);

            // Display the diff
            sync::display_conflict(
                order_name,
                &result.name,
                &result.target,
                prompt_diff,
                &ctx.home_dir,
            );

            // Determine action
            let action = match mode {
                SyncMode::ApplySourceToTargetAll => SyncAction::ApplySourceToTarget,
                SyncMode::ApplyTargetToSourceAll => {
                    if can_pull {
                        SyncAction::ApplyTargetToSource
                    } else {
                        let reason = if structural_mismatch {
                            "target structure does not match Source"
                        } else {
                            "from_config contains logic"
                        };
                        log::warn(&format!(
                            "Cannot apply Target -> Source for {}:{} ({}), skipping",
                            order_name, result.name, reason
                        ));
                        SyncAction::Skip
                    }
                }
                SyncMode::Interactive => {
                    if ctx.dry_run {
                        let pull_note = if can_pull {
                            ", Target -> Source available"
                        } else {
                            ", manual merge needed"
                        };
                        log::info_important(&format!("[dry-run] would prompt{}", pull_note));
                        SyncAction::Skip
                    } else {
                        let annotation = if structural_mismatch {
                            None
                        } else {
                            let deployed_bytes = std::fs::read(&result.target).unwrap_or_default();
                            sync::compute_file_annotation(
                                snapshot.as_deref(),
                                result.content.as_bytes(),
                                &deployed_bytes,
                            )
                        };
                        prompter.ask_sync_action(
                            order_name,
                            &result.name,
                            &result.target,
                            &diff_result,
                            can_pull,
                            annotation,
                        )
                    }
                }
            };

            match action {
                SyncAction::ApplySourceToTarget => match write_result(result, ctx.dry_run) {
                    Ok(()) => {
                        if !ctx.dry_run {
                            log::success(&format!(
                                "Applied Source -> Target for {}:{}",
                                order_name, result.name
                            ));
                            if !mode_reconciled {
                                source_to_target += 1;
                            }
                            refresh_snapshot_for_result(ctx, order_name, result, &mut build_errors);
                        }
                    }
                    Err(e) => {
                        log::error(&format!(
                            "Failed to apply Source -> Target for {}:{}: {}",
                            order_name, result.name, e
                        ));
                        build_errors += 1;
                    }
                },
                SyncAction::ApplyTargetToSource => {
                    /// Whether the pull landed anything in Source, and if
                    /// so whether every needed key was written back.
                    enum PullStatus {
                        Applied { fully: bool },
                        NothingApplied,
                    }

                    let pull_result = if result.is_plaintext {
                        sync::pull_build_result(result, ctx.dry_run)
                            .map(|()| PullStatus::Applied { fully: true })
                    } else {
                        let format = result.format;
                        sync::pull_from_config(
                            ctx,
                            order_name,
                            *file_entry_index,
                            &result.target,
                            format,
                            ctx.dry_run,
                        )
                        .map(|report| {
                            if report.any_applied() {
                                PullStatus::Applied {
                                    fully: report.unapplied.is_empty(),
                                }
                            } else {
                                PullStatus::NothingApplied
                            }
                        })
                    };

                    match pull_result {
                        Ok(PullStatus::Applied { fully }) => {
                            if !ctx.dry_run {
                                log::success(&format!(
                                    "Applied Target -> Source for {}:{}",
                                    order_name, result.name
                                ));
                                target_to_source += 1;
                                if fully {
                                    refresh_snapshot_for_result(
                                        ctx,
                                        order_name,
                                        result,
                                        &mut build_errors,
                                    );
                                } else {
                                    // Partial write-back: leave the snapshot
                                    // alone so the remaining keys resurface
                                    // as a conflict instead of being masked.
                                    log::warn(&format!(
                                        "Some keys in {}:{} could not be written back to order.ncl; they remain unresolved",
                                        order_name, result.name
                                    ));
                                }
                            }
                        }
                        Ok(PullStatus::NothingApplied) => {
                            if !ctx.dry_run {
                                log::warn(&format!(
                                    "Cannot apply Target -> Source for {}:{} (no key could be rewritten in order.ncl), skipping",
                                    order_name, result.name
                                ));
                            }
                            skipped += 1;
                        }
                        Err(e) => {
                            log::error(&format!(
                                "Failed to apply Target -> Source for {}:{}: {}",
                                order_name, result.name, e
                            ));
                            build_errors += 1;
                        }
                    }
                }
                SyncAction::Skip => {
                    skipped += 1;
                }
                SyncAction::Quit => {
                    log::info("Sync aborted by user");
                    break 'resources;
                }
            }
        }
    }

    finish_deferred_modes(
        &deferred_modes,
        ctx.dry_run,
        &mut source_to_target,
        &mut build_errors,
    );
    finish_directory_modes(
        &directory_modes,
        &mut directory_access,
        ctx.dry_run,
        &mut source_to_target,
        &mut build_errors,
    );
    if build_errors > 0 {
        anyhow::bail!("sync failed with {build_errors} error(s)");
    }

    if crate::interrupt::interrupted() {
        anyhow::bail!("Sync interrupted; directory permissions restored");
    }

    // Summary
    if !ctx.dry_run {
        log::success(&format!(
            "Sync complete: {} Source -> Target, {} Target -> Source, {} skipped",
            source_to_target, target_to_source, skipped
        ));
    }

    Ok(())
}

fn finish_directory_modes(
    entries: &[(&BuildResult, bool)],
    access: &mut crate::resources::DirectoryAccess,
    dry_run: bool,
    applied: &mut usize,
    errors: &mut usize,
) {
    // Children before parents, including directory resources in different Orders.
    let mut entries = entries.to_vec();
    entries.sort_by_key(|(result, _)| std::cmp::Reverse(result.target.components().count()));
    for (result, _) in &entries {
        if dry_run {
            if let Some(mode) = result.mode {
                log::info(&format!(
                    "Would enforce directory mode {:04o} on {}",
                    mode,
                    result.target.display()
                ));
            }
        } else {
            access.set_final_mode(&result.target, result.mode);
        }
    }
    // Restore temporary access and enforce declared modes in one deepest-first
    // pass, including unselected ancestors and newly created directories.
    if let Err(error) = access.finish() {
        *errors += 1;
        log::error(&format!("Could not finalize directory modes: {error:#}"));
    } else if !dry_run {
        for (result, already_applied) in entries {
            if access.mode_changed(&result.target) && !already_applied {
                *applied += 1;
                log::success(&format!(
                    "Applied declared directory mode for {}",
                    result.target.display()
                ));
            }
        }
    }
}

fn finish_deferred_modes(
    entries: &[(&str, &BuildResult)],
    dry_run: bool,
    source_to_target: &mut usize,
    errors: &mut usize,
) {
    for (order, result) in entries {
        match reconcile_file_mode(result, dry_run) {
            Ok(_) if !dry_run => {
                *source_to_target += 1;
                log::success(&format!(
                    "Applied declared file mode for {}:{}",
                    order, result.name
                ));
            }
            Ok(_) => {}
            Err(error) => {
                *errors += 1;
                log::error(&format!(
                    "Failed to reconcile file mode for {}:{}: {}",
                    order, result.name, error
                ));
            }
        }
    }
}

fn compute_diff_with_base_for_result(
    result: &BuildResult,
    snapshot: Option<&[u8]>,
    structural_mismatch: bool,
) -> Option<crate::diff::DiffResult> {
    let base = std::str::from_utf8(snapshot?).ok()?;

    if structural_mismatch {
        return None;
    }

    if result.is_plaintext {
        let source_path = result.source_path.as_ref()?;
        if result.source_is_directory() {
            return None;
        }
        let source_content = std::fs::read_to_string(source_path).ok()?;
        let deployed = std::fs::read_to_string(&result.target).ok()?;
        Some(diff_configs_with_base(
            nickel::Format::Plaintext,
            &source_content,
            &deployed,
            base,
            &result.ignore_keys,
        ))
    } else {
        let deployed = std::fs::read_to_string(&result.target).ok()?;
        Some(diff_configs_with_base(
            result.format,
            &result.content,
            &deployed,
            base,
            &result.ignore_keys,
        ))
    }
}

fn invalidate_content_base(
    ctx: &Context,
    order_name: &str,
    result: &BuildResult,
    errors: &mut usize,
) {
    if let Err(error) = ctx.state.forget_content(order_name, &result.target) {
        *errors += 1;
        log::error(&format!("Could not invalidate content snapshot: {error:#}"));
    }
}

/// Refresh snapshots for a successfully-deployed `BuildResult`.
///
/// For single-file entries, writes one snapshot keyed by the deployed
/// target. For directory entries (plaintext source is a directory),
/// writes snapshots only for files that blend builds from Source/local
/// overlays. Target-only files are outside blend's ownership boundary and
/// intentionally ignored.
/// Symlink entries are skipped — they have no semantic content.
///
/// Failures are logged at warn level and are non-fatal — the deploy
/// already succeeded, and the next no-op confirm will heal the
/// snapshot.
fn refresh_snapshot_for_result(
    ctx: &Context,
    order_name: &str,
    result: &BuildResult,
    errors: &mut usize,
) {
    if result.kind != nickel::ResourceKind::Content {
        invalidate_content_base(ctx, order_name, result, errors);
        return;
    }

    if result.is_plaintext && result.source_is_directory() {
        let Some(source_path) = &result.source_path else {
            return;
        };
        let managed_files = match &result.validated_source {
            Some(compose::SourceManifest::Tree(files)) => Ok(files.clone()),
            _ => build_glob_set(&result.exclude_patterns).and_then(|exclude| {
                collect_merged_files(source_path, result.local_dir.as_deref(), exclude.as_ref())
            }),
        };
        let managed_files = match managed_files {
            Ok(files) => files,
            Err(error) => {
                log::warn(&format!(
                    "snapshot refresh: could not inventory {}: {error:#}",
                    source_path.display()
                ));
                return;
            }
        };

        for managed_file in managed_files {
            let target_path = result.target.join(&managed_file.rel_path);
            let bytes = match std::fs::read(&target_path) {
                Ok(b) => b,
                Err(e) => {
                    log::warn(&format!(
                        "snapshot refresh: failed to read {}: {}",
                        target_path.display(),
                        e
                    ));
                    continue;
                }
            };
            if let Err(e) = ctx.state.write(order_name, &target_path, &bytes) {
                log::warn(&format!(
                    "snapshot refresh: failed to write snapshot for {}: {}",
                    target_path.display(),
                    e
                ));
            }
        }
        return;
    }

    // Single-file entry — read deployed bytes and snapshot.
    let bytes = match std::fs::read(&result.target) {
        Ok(b) => b,
        Err(e) => {
            log::warn(&format!(
                "snapshot refresh: failed to read {}: {}",
                result.target.display(),
                e
            ));
            return;
        }
    };
    if let Err(e) = ctx.state.write(order_name, &result.target, &bytes) {
        log::warn(&format!(
            "snapshot refresh: failed to write snapshot for {}: {}",
            result.target.display(),
            e
        ));
    }
}

/// Check if Target -> Source can be applied automatically for a build result.
/// from_file entries can always be pulled. from_config entries use context-aware
/// shadow walk — Target -> Source is possible if the walk reaches literal leaves.
fn can_auto_pull(
    ctx: &Context,
    order_name: &str,
    result: &BuildResult,
    file_entry_index: usize,
) -> bool {
    if result.is_plaintext {
        return true;
    }

    let order_dir = ctx.orders_dir.join(order_name);
    let ncl_path = order_dir.join("order.ncl");

    let source = match std::fs::read_to_string(&ncl_path) {
        Ok(s) => s,
        Err(_) => return false,
    };

    match nickel::ast_utils::locate_from_config(&source, file_entry_index, &ctx.metadata) {
        Ok(result) => result.has_any_rewritable(),
        Err(_) => false,
    }
}

/// Build an order and return results with file_entry_index for each result
fn build_order_with_indices(
    ctx: &Context,
    order_name: &str,
    ownership: &crate::resources::Ownership,
) -> anyhow::Result<Vec<(BuildResult, usize)>> {
    let order_dir = ctx.orders_dir.join(order_name);
    let ncl_path = order_dir.join("order.ncl");

    if !ncl_path.exists() {
        return Ok(vec![]);
    }

    let evaluator = nickel::NickelEvaluator::new(&ctx.metadata);
    let order = evaluator.evaluate(&ncl_path)?;
    crate::interrupt::check()?;

    if !order.should_apply(&ctx.metadata.os, &ctx.metadata.arch, &ctx.metadata.hostname) {
        return Ok(vec![]);
    }

    let mut results = Vec::new();
    let global_ignore = order.global_ignore();
    let global_prefix = order.global_prefix();

    for (file_entry_index, file_entry) in order.blend.files.iter().enumerate() {
        crate::interrupt::check()?;
        if !file_entry.should_apply(&ctx.metadata.os, &ctx.metadata.arch, &ctx.metadata.hostname) {
            if ctx.verbose {
                log::info(&format!(
                    "Skipping file {} (when condition not met)",
                    file_entry.name,
                ));
            }
            continue;
        }

        let mut ignore_keys: Vec<String> = global_ignore.to_vec();
        ignore_keys.extend(file_entry.ignore.iter().cloned());

        for target_path in file_entry.target_paths(global_prefix) {
            let expanded_target = ctx.expand_path(&target_path);
            crate::resources::assert_managed_ancestors(ownership, &expanded_target)?;
            let mut result = compose::build_file_entry_pub(
                ctx,
                &order_dir,
                file_entry_index,
                order.entry_has_resolution_semantics(file_entry_index),
                file_entry,
                expanded_target,
                ignore_keys.clone(),
            )?;
            crate::resources::bind_source_manifest(ownership, &mut result)?;
            results.push((result, file_entry_index));
        }
    }

    Ok(results)
}
