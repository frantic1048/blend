use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use anyhow::{Context as AnyhowContext, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use walkdir::WalkDir;

use crate::context::Context;
use crate::formats::get_renderer;
use crate::fs_node::{NodeKind, node_kind};
use crate::immutable;
use crate::nickel::resolution::ResourceDisposition;
use crate::nickel::{FileEntry, Format, NickelEvaluator, Order};
use crate::output::log;

#[derive(Debug, Default)]
pub struct FileResolution {
    pub automatic_source_paths: HashSet<crate::nickel::key_path::KeyPath>,
    pub manual_resolution_paths: HashSet<crate::nickel::key_path::KeyPath>,
    pub resource_disposition: ResourceDisposition,
}

/// Discover orders in the orders directory
pub fn discover_orders(orders_dir: &Path) -> HashSet<String> {
    let mut orders = HashSet::new();

    let Ok(entries) = std::fs::read_dir(orders_dir) else {
        return orders;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        // Skip hidden directories
        if name.starts_with('.') {
            continue;
        }

        // Check for order.ncl
        if path.join("order.ncl").exists() {
            orders.insert(name.to_string());
        }
    }

    orders
}

/// Result of building a single file entry target
#[derive(Debug)]
pub struct BuildResult {
    /// Target path (expanded)
    pub target: PathBuf,
    /// Rendered content (empty for plaintext)
    pub content: String,
    /// Whether this is a plaintext copy
    pub is_plaintext: bool,
    /// Source path for plaintext copies
    pub source_path: Option<PathBuf>,
    /// Name from FileEntry
    pub name: String,
    /// Effective format from FileEntry (explicit or inferred)
    pub format: Format,
    /// Merged ignore keys (global + per-file)
    pub ignore_keys: Vec<String>,
    /// Whether this entry should be symlinked instead of copied
    pub is_symlink: bool,
    /// Canonical (absolute) source path for symlink entries
    pub canonical_source: Option<PathBuf>,
    /// Glob patterns to exclude when shipping a directory
    pub exclude_patterns: Vec<String>,
    /// Path to the local overlay directory (if set and source is a directory)
    pub local_dir: Option<PathBuf>,
    /// Whether to set the OS immutable flag on the deployed file
    pub immutable: bool,
    /// Unix permission mode enforced on managed regular target files.
    pub mode: Option<u32>,
    /// Structured paths whose declarations are authoritative and can be
    /// reconciled without prompting.
    pub automatic_source_paths: HashSet<crate::nickel::key_path::KeyPath>,
    /// Structured paths for which a resolver explicitly returned
    /// `'Unresolved`. These require a manual declaration edit.
    pub manual_resolution_paths: HashSet<crate::nickel::key_path::KeyPath>,
    /// Resource-root ownership/existence semantics.
    pub resource_disposition: ResourceDisposition,
}

/// Build a single order, returning results for all file entries and targets
pub fn build_order(ctx: &Context, order_name: &str) -> Result<Vec<BuildResult>> {
    let order_dir = ctx.orders_dir.join(order_name);
    let ncl_path = order_dir.join("order.ncl");

    if !ncl_path.exists() {
        return Ok(vec![]);
    }

    let evaluator = NickelEvaluator::new(&ctx.metadata);
    let order = evaluator.evaluate(&ncl_path)?;

    // Check if order should be applied for this system
    if !order.should_apply(&ctx.metadata.os, &ctx.metadata.arch, &ctx.metadata.hostname) {
        return Ok(vec![]);
    }

    let mut results = Vec::new();
    let global_ignore = order.global_ignore();
    let global_prefix = order.global_prefix();

    for (file_entry_index, file_entry) in order.blend.files.iter().enumerate() {
        // Check per-file condition
        if !file_entry.should_apply(&ctx.metadata.os, &ctx.metadata.arch, &ctx.metadata.hostname) {
            if ctx.verbose {
                log::info(&format!(
                    "Skipping file {} (when condition not met)",
                    file_entry.name,
                ));
            }
            continue;
        }

        // Merge ignore keys: global + per-file
        let mut ignore_keys: Vec<String> = global_ignore.to_vec();
        ignore_keys.extend(file_entry.ignore.iter().cloned());

        // Build for each target prefix (file-level overrides global)
        for target_path in file_entry.target_paths(global_prefix) {
            let expanded_target = ctx.expand_path(&target_path);
            let (evaluated_entry, resolution) = resolve_file_entry(
                ctx,
                &order_dir,
                file_entry_index,
                order.entry_has_resolution_semantics(file_entry_index),
                file_entry,
                &expanded_target,
            )?;
            let result = build_file_entry(
                &order_dir,
                &evaluated_entry,
                expanded_target,
                ignore_keys.clone(),
                resolution,
            )?;
            results.push(result);
        }
    }

    Ok(results)
}

/// Build a single file entry to a specific target (public wrapper)
pub fn build_file_entry_pub(
    ctx: &Context,
    order_dir: &Path,
    file_entry_index: usize,
    has_resolution_semantics: bool,
    entry: &FileEntry,
    target: PathBuf,
    ignore_keys: Vec<String>,
) -> Result<BuildResult> {
    let (evaluated_entry, resolution) = resolve_file_entry(
        ctx,
        order_dir,
        file_entry_index,
        has_resolution_semantics,
        entry,
        &target,
    )?;
    build_file_entry(order_dir, &evaluated_entry, target, ignore_keys, resolution)
}

pub fn resolve_file_entry(
    ctx: &Context,
    order_dir: &Path,
    file_entry_index: usize,
    has_resolution_semantics: bool,
    entry: &FileEntry,
    target: &Path,
) -> Result<(FileEntry, FileResolution)> {
    if !has_resolution_semantics {
        return Ok((entry.clone(), FileResolution::default()));
    }
    let evaluator = NickelEvaluator::new(&ctx.metadata);
    let target_value = read_target_config(entry, target)?;
    let plan = evaluator.resolve_config(
        &order_dir.join("order.ncl"),
        file_entry_index,
        target_value.as_ref(),
    )?;
    let mut evaluated_entry = entry.clone();
    evaluated_entry.from_config = Some(plan.source.unwrap_or(serde_json::Value::Null));
    Ok((
        evaluated_entry,
        FileResolution {
            automatic_source_paths: plan.automatic,
            manual_resolution_paths: plan.manual,
            resource_disposition: plan.resource,
        },
    ))
}

/// Build a single file entry to a specific target
fn build_file_entry(
    order_dir: &Path,
    entry: &FileEntry,
    target: PathBuf,
    ignore_keys: Vec<String>,
    resolution: FileResolution,
) -> Result<BuildResult> {
    let FileResolution {
        automatic_source_paths,
        manual_resolution_paths,
        resource_disposition,
    } = resolution;
    let mode = entry.parsed_mode()?;
    if mode.is_some() && !cfg!(unix) {
        return Err(anyhow::anyhow!(
            "File entry '{}': 'mode' is only supported on Unix targets",
            entry.name
        ));
    }
    if let Some(file) = &entry.from_file {
        let source_path = order_dir.join(file);
        if !source_path.exists() {
            return Err(anyhow::anyhow!(
                "File entry '{}': source file not found at {}",
                entry.name,
                source_path.display()
            ));
        }

        // Resolve local overlay directory
        let local_dir = if let Some(local) = &entry.local {
            let ld = order_dir.join(local);
            // Auto-create local dir if it doesn't exist
            if !ld.exists() {
                std::fs::create_dir_all(&ld).with_context(|| {
                    format!("Failed to create local overlay directory {}", ld.display())
                })?;
            }
            Some(ld)
        } else {
            None
        };

        if entry.symlink {
            let canonical = source_path.canonicalize().with_context(|| {
                format!(
                    "Failed to canonicalize source path {}",
                    source_path.display()
                )
            })?;
            return Ok(BuildResult {
                target,
                content: String::new(),
                is_plaintext: true,
                source_path: Some(source_path),
                name: entry.name.clone(),
                format: entry.effective_format(),
                ignore_keys,
                is_symlink: true,
                canonical_source: Some(canonical),
                exclude_patterns: entry.exclude.clone(),
                local_dir,
                immutable: entry.immutable,
                mode,
                automatic_source_paths,
                manual_resolution_paths,
                resource_disposition,
            });
        }

        return Ok(BuildResult {
            target,
            content: String::new(),
            is_plaintext: true,
            source_path: Some(source_path),
            name: entry.name.clone(),
            format: entry.effective_format(),
            ignore_keys,
            is_symlink: false,
            canonical_source: None,
            exclude_patterns: entry.exclude.clone(),
            local_dir,
            immutable: entry.immutable,
            mode,
            automatic_source_paths,
            manual_resolution_paths,
            resource_disposition,
        });
    }

    if let Some(config) = &entry.from_config {
        let format = entry.effective_format();
        let renderer = get_renderer(format);
        let content = if resource_disposition == ResourceDisposition::Present {
            renderer.render(config)?
        } else {
            String::new()
        };

        return Ok(BuildResult {
            target,
            content,
            is_plaintext: false,
            source_path: None,
            name: entry.name.clone(),
            format,
            ignore_keys,
            is_symlink: false,
            canonical_source: None,
            exclude_patterns: vec![],
            local_dir: None,
            immutable: entry.immutable,
            mode,
            automatic_source_paths,
            manual_resolution_paths,
            resource_disposition,
        });
    }

    // Unreachable after resolve_defaults validation
    Err(anyhow::anyhow!(
        "File entry '{}' has neither 'from_file' nor 'from_config'",
        entry.name,
    ))
}

fn read_target_config(entry: &FileEntry, target: &Path) -> Result<Option<serde_json::Value>> {
    let Some(_) = &entry.from_config else {
        return Ok(None);
    };
    if !target.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(target)
        .with_context(|| format!("Failed to read {}", target.display()))?;
    let value = get_renderer(entry.effective_format())
        .parse(&content)
        .with_context(|| {
            format!(
                "Target {} is malformed and cannot be used as a resolver observation",
                target.display()
            )
        })?;
    Ok(Some(value))
}

fn ensure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("Failed to create directory {}", path.display()))?;
    Ok(())
}

fn remove_exact_node(path: &Path, dry_run: bool) -> Result<()> {
    let Some(kind) = node_kind(path)? else {
        return Ok(());
    };
    if dry_run {
        log::info(&format!("Would remove {kind} {}", path.display()));
        return Ok(());
    }
    if kind == NodeKind::Directory {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("Failed to remove directory {}", path.display()))?;
    } else {
        std::fs::remove_file(path)
            .with_context(|| format!("Failed to remove {kind} {}", path.display()))?;
    }
    Ok(())
}

fn prepare_exact_node(path: &Path, expected: NodeKind, dry_run: bool) -> Result<()> {
    if let Some(actual) = node_kind(path)?
        && actual != expected
    {
        remove_exact_node(path, dry_run)?;
    }
    Ok(())
}

fn prepare_managed_parent_dirs(root: &Path, relative: &Path, dry_run: bool) -> Result<()> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        prepare_exact_node(&current, NodeKind::Directory, dry_run)?;
        if !dry_run {
            ensure_dir(&current)?;
        }
    }
    Ok(())
}

/// Remove OS immutable flag from a file so it can be overwritten.
fn remove_immutable_flag(path: &Path, warn_on_failure: bool) -> Result<()> {
    immutable::clear(path, warn_on_failure)
}

/// Set OS immutable flag on a file to prevent modification.
fn set_immutable_flag(path: &Path) -> Result<()> {
    immutable::set(path)
}

/// Set immutable flags on all files within a directory.
fn set_immutable_flag_recursive(dir: &Path) -> Result<()> {
    for entry in WalkDir::new(dir).min_depth(1) {
        let entry = entry?;
        if entry.file_type().is_file() {
            set_immutable_flag(entry.path())?;
        }
    }
    Ok(())
}

/// Remove immutable flags from all files within a directory.
fn remove_immutable_flag_recursive(dir: &Path, warn_on_failure: bool) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in WalkDir::new(dir).min_depth(1) {
        let entry = entry?;
        if entry.file_type().is_file() {
            remove_immutable_flag(entry.path(), warn_on_failure)?;
        }
    }
    Ok(())
}

/// Write build result to target
pub fn write_result(result: &BuildResult, dry_run: bool) -> Result<()> {
    match result.resource_disposition {
        ResourceDisposition::Absent => {
            if result.target.exists() && !dry_run {
                remove_immutable_flag(&result.target, result.immutable)?;
            }
            return remove_exact_node(&result.target, dry_run);
        }
        ResourceDisposition::Unmanaged | ResourceDisposition::Unresolved => return Ok(()),
        ResourceDisposition::Present => {}
    }

    if result.is_symlink {
        if let Some(canonical) = &result.canonical_source {
            return create_symlink(canonical, &result.target, dry_run);
        }
        return Err(anyhow::anyhow!(
            "Symlink entry '{}' has no canonical source path",
            result.name
        ));
    }

    let is_plaintext_dir = result.is_plaintext
        && result
            .source_path
            .as_ref()
            .is_some_and(|source_path| source_path.is_dir());

    if result.is_plaintext {
        if let Some(source_path) = &result.source_path {
            if source_path.is_dir() {
                let exclude = build_glob_set(&result.exclude_patterns)?;
                copy_directory(
                    source_path,
                    &result.target,
                    result.local_dir.as_deref(),
                    exclude.as_ref(),
                    result.immutable,
                    result.mode,
                    dry_run,
                )?;
            } else {
                copy_file(
                    source_path,
                    &result.target,
                    result.mode,
                    result.immutable,
                    dry_run,
                )?;
            }
        }
    } else {
        if dry_run {
            prepare_exact_node(&result.target, NodeKind::File, true)?;
            log::info(&format!("Would write to {}", result.target.display()));
            if result.immutable {
                log::info(&format!(
                    "Would set immutable flag on {}",
                    result.target.display()
                ));
            }
            return Ok(());
        }

        atomic_replace_file(
            &result.target,
            result.mode,
            None,
            result.immutable,
            |file| {
                file.write_all(result.content.as_bytes()).with_context(|| {
                    format!(
                        "Failed to write replacement for {}",
                        result.target.display()
                    )
                })
            },
        )?;
    }

    // Set immutable flag after successful write
    if result.immutable && !dry_run && !is_plaintext_dir {
        if result.target.is_dir() {
            set_immutable_flag_recursive(&result.target)?;
        } else {
            set_immutable_flag(&result.target)?;
        }
    }

    Ok(())
}

/// Create a symlink at target pointing to source
fn create_symlink(source: &Path, target: &Path, dry_run: bool) -> Result<()> {
    remove_exact_node(target, dry_run)?;
    if dry_run {
        log::info(&format!(
            "Would symlink {} -> {}",
            target.display(),
            source.display()
        ));
        return Ok(());
    }

    // Ensure parent directory exists
    if let Some(parent) = target.parent() {
        ensure_dir(parent)?;
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(source, target).with_context(|| {
        format!(
            "Failed to create symlink {} -> {}",
            target.display(),
            source.display()
        )
    })?;

    #[cfg(not(unix))]
    return Err(anyhow::anyhow!(
        "Symlinks are only supported on Unix platforms"
    ));

    Ok(())
}

/// Copy a single file to target
fn copy_file(
    source: &Path,
    target: &Path,
    mode: Option<u32>,
    immutable: bool,
    dry_run: bool,
) -> Result<()> {
    if dry_run {
        prepare_exact_node(target, NodeKind::File, true)?;
        log::info(&format!(
            "Would copy {} to {}",
            source.display(),
            target.display()
        ));
        return Ok(());
    }

    let source_permissions = source
        .metadata()
        .with_context(|| format!("Failed to inspect {}", source.display()))?
        .permissions();
    atomic_replace_file(target, mode, Some(source_permissions), immutable, |file| {
        let mut source_file = std::fs::File::open(source)
            .with_context(|| format!("Failed to open {}", source.display()))?;
        std::io::copy(&mut source_file, file).with_context(|| {
            format!(
                "Failed to copy {} to {}",
                source.display(),
                target.display()
            )
        })?;
        Ok(())
    })
}

/// Construct a complete regular-file replacement beside the Target, then
/// atomically rename it into place. The temporary file is collision-resistant
/// and is removed automatically on every pre-rename error path.
fn atomic_replace_file(
    target: &Path,
    mode: Option<u32>,
    fallback_permissions: Option<std::fs::Permissions>,
    immutable: bool,
    write: impl FnOnce(&mut std::fs::File) -> Result<()>,
) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Target has no parent: {}", target.display()))?;
    ensure_dir(parent)?;

    let existing_kind = node_kind(target)?;
    let existing_permissions = if existing_kind == Some(NodeKind::File) {
        Some(
            target
                .symlink_metadata()
                .with_context(|| format!("Failed to inspect {}", target.display()))?
                .permissions(),
        )
    } else {
        None
    };

    #[cfg(unix)]
    let desired_permissions = mode
        .map(std::fs::Permissions::from_mode)
        .or(existing_permissions)
        .or(fallback_permissions);
    #[cfg(not(unix))]
    let desired_permissions = existing_permissions.or(fallback_permissions);

    let mut builder = tempfile::Builder::new();
    builder.prefix(".blend-").suffix(".tmp");
    #[cfg(unix)]
    {
        let creation_permissions = desired_permissions
            .clone()
            .unwrap_or_else(|| std::fs::Permissions::from_mode(0o666));
        builder.permissions(creation_permissions);
    }
    #[cfg(not(unix))]
    if let Some(permissions) = desired_permissions.clone() {
        builder.permissions(permissions);
    }

    let mut replacement = builder.tempfile_in(parent).with_context(|| {
        format!(
            "Failed to create temporary replacement beside {}",
            target.display()
        )
    })?;
    write(replacement.as_file_mut())?;
    replacement.as_file_mut().flush().with_context(|| {
        format!(
            "Failed to flush temporary replacement for {}",
            target.display()
        )
    })?;
    if let Some(permissions) = desired_permissions {
        replacement
            .as_file()
            .set_permissions(permissions)
            .with_context(|| {
                format!(
                    "Failed to set permissions on replacement for {}",
                    target.display()
                )
            })?;
    }

    // Closing the handle before rename keeps the replacement path portable and
    // ensures all buffered userspace writes completed before it becomes live.
    let replacement = replacement.into_temp_path();

    match existing_kind {
        Some(NodeKind::Directory) => {
            remove_immutable_flag_recursive(target, immutable)?;
            remove_exact_node(target, false)?;
        }
        Some(NodeKind::File) => {
            #[cfg(unix)]
            {
                match open_regular_file_no_follow(target) {
                    Ok(existing) => immutable::clear_file(&existing, target, immutable)?,
                    Err(_) => {
                        #[cfg(not(target_os = "linux"))]
                        if immutable {
                            immutable::clear_no_follow(target)?;
                        }
                        // Linux cannot inspect/update flags through a usable
                        // ioctl descriptor when read access is denied. The
                        // declaration does not imply that the existing inode
                        // is immutable: attempt rename without modifying it.
                        // The kernel rejects replacement if it really carries
                        // the immutable flag, preserving the old Target.
                    }
                }
            }
            #[cfg(not(unix))]
            remove_immutable_flag(target, immutable)?;
        }
        Some(NodeKind::Symlink | NodeKind::Other) => remove_exact_node(target, false)?,
        None => {}
    }

    replacement.persist(target).map_err(|error| {
        anyhow::anyhow!(
            "Failed to atomically replace {}: {}",
            target.display(),
            error.error
        )
    })?;
    Ok(())
}

#[cfg(unix)]
fn open_regular_file_no_follow(path: &Path) -> Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .with_context(|| {
            format!(
                "Failed to open regular file {} without following links",
                path.display()
            )
        })?;
    if !file
        .metadata()
        .with_context(|| format!("Failed to inspect open file {}", path.display()))?
        .file_type()
        .is_file()
    {
        return Err(anyhow::anyhow!(
            "Target changed type while reconciling {}",
            path.display()
        ));
    }
    Ok(file)
}

/// Return managed regular Target files whose declared Unix mode has drifted.
pub fn mode_mismatches(result: &BuildResult) -> Result<Vec<(PathBuf, u32)>> {
    let Some(expected) = result.mode else {
        return Ok(Vec::new());
    };

    #[cfg(not(unix))]
    return Err(anyhow::anyhow!("'mode' is only supported on Unix targets"));

    #[cfg(unix)]
    {
        let targets = if result.is_plaintext
            && result
                .source_path
                .as_ref()
                .is_some_and(|source_path| source_path.is_dir())
        {
            if node_kind(&result.target)? != Some(NodeKind::Directory) {
                return Ok(Vec::new());
            }
            let source = result.source_path.as_ref().expect("checked above");
            let exclude = build_glob_set(&result.exclude_patterns)?;
            let mut targets = Vec::new();
            for file in collect_merged_files(source, result.local_dir.as_deref(), exclude.as_ref())?
            {
                let mut parent = result.target.clone();
                let mut valid = true;
                if let Some(relative_parent) = file.rel_path.parent() {
                    for component in relative_parent.components() {
                        parent.push(component);
                        if node_kind(&parent)? != Some(NodeKind::Directory) {
                            valid = false;
                            break;
                        }
                    }
                }
                if valid {
                    targets.push(result.target.join(file.rel_path));
                }
            }
            targets
        } else {
            vec![result.target.clone()]
        };

        let mut mismatches = Vec::new();
        for target in targets {
            if node_kind(&target)? != Some(NodeKind::File) {
                continue;
            }
            if let Some(actual) = file_mode_mismatch(&target, expected)? {
                mismatches.push((target, actual));
            }
        }
        Ok(mismatches)
    }
}

/// Return the actual Unix mode when a regular file differs from `expected`.
pub fn file_mode_mismatch(path: &Path, expected: u32) -> Result<Option<u32>> {
    if node_kind(path)? != Some(NodeKind::File) {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        let actual = path
            .symlink_metadata()
            .with_context(|| format!("Failed to inspect {}", path.display()))?
            .permissions()
            .mode()
            & 0o7777;
        Ok((actual != expected).then_some(actual))
    }
    #[cfg(not(unix))]
    {
        let _ = expected;
        Err(anyhow::anyhow!("'mode' is only supported on Unix targets"))
    }
}

/// Enforce declared file modes independently of content reconciliation.
/// Returns whether any regular Target file needed a mode correction.
pub fn reconcile_file_mode(result: &BuildResult, dry_run: bool) -> Result<bool> {
    let Some(expected) = result.mode else {
        return Ok(false);
    };
    let mismatches = mode_mismatches(result)?;
    for (target, actual) in &mismatches {
        if dry_run {
            log::info(&format!(
                "Would change mode of {} from {:04o} to {:04o}",
                target.display(),
                actual,
                expected
            ));
            continue;
        }

        #[cfg(unix)]
        {
            reconcile_regular_file_mode(target, expected, result.immutable)?;
        }
    }
    Ok(!mismatches.is_empty())
}

#[cfg(unix)]
fn reconcile_regular_file_mode(path: &Path, expected: u32, immutable: bool) -> Result<()> {
    let file = match open_regular_file_no_follow(path) {
        Ok(file) => file,
        Err(_) if !immutable => {
            set_file_mode_no_follow(path, expected)?;
            return Ok(());
        }
        Err(_) => {
            #[cfg(target_os = "macos")]
            {
                immutable::clear_no_follow(path)?;
                // Restoration must not require read access, even when the
                // requested mode remains unreadable. Also restore on failure.
                let mode_result = set_file_mode_no_follow(path, expected);
                let flag_result = immutable::set_no_follow(path);
                mode_result?;
                flag_result?;
            }
            #[cfg(not(target_os = "macos"))]
            {
                // An immutable declaration does not imply an existing flag.
                // chmod through the verified inode first: this succeeds for
                // an owned unreadable file whose immutable flag is absent.
                set_file_mode_no_follow(path, expected)?;
                match open_regular_file_no_follow(path) {
                    Ok(file) => immutable::set_file(&file, path)?,
                    Err(error) => log::warn(&format!(
                        "Could not set immutable flag on {} after mode repair: {}",
                        path.display(),
                        error
                    )),
                }
            }
            return Ok(());
        }
    };
    let actual = file
        .metadata()
        .with_context(|| format!("Failed to inspect open file {}", path.display()))?
        .permissions()
        .mode()
        & 0o7777;
    if actual == expected {
        if immutable {
            immutable::set_file(&file, path)?;
        }
        return Ok(());
    }
    immutable::clear_file(&file, path, immutable)?;
    file.set_permissions(std::fs::Permissions::from_mode(expected))
        .with_context(|| format!("Failed to set mode on {}", path.display()))?;
    if immutable {
        immutable::set_file(&file, path)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn set_file_mode_no_follow(path: &Path, mode: u32) -> Result<()> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .with_context(|| format!("Failed to open {} without following links", path.display()))?;
    if !file
        .metadata()
        .with_context(|| format!("Failed to inspect open file {}", path.display()))?
        .file_type()
        .is_file()
    {
        anyhow::bail!("Target changed type while reconciling {}", path.display());
    }

    // Linux's original fchmodat syscall ignores flags, while glibc rejects
    // AT_SYMLINK_NOFOLLOW on kernels without fchmodat2. A procfs path to a
    // held O_PATH descriptor names the already-verified inode, so chmod cannot
    // be redirected by replacing the original path with a symlink.
    let descriptor_path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
    std::fs::set_permissions(&descriptor_path, std::fs::Permissions::from_mode(mode)).with_context(
        || {
            format!(
                "Failed to set mode on {} without following links",
                path.display()
            )
        },
    )
}

#[cfg(all(unix, not(target_os = "linux")))]
fn set_file_mode_no_follow(path: &Path, mode: u32) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path_bytes = CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("Path contains an interior NUL byte: {}", path.display()))?;
    let result = unsafe {
        libc::fchmodat(
            libc::AT_FDCWD,
            path_bytes.as_ptr(),
            mode as libc::mode_t,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == -1 {
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!(
                "Failed to set mode on {} without following links",
                path.display()
            )
        });
    }
    Ok(())
}

/// Build a GlobSet from a list of glob pattern strings.
/// Returns None if the list is empty.
pub fn build_glob_set(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        builder
            .add(Glob::new(pat).with_context(|| format!("Invalid exclude glob pattern: {pat}"))?);
    }
    Ok(Some(builder.build()?))
}

/// Represents a file in a merged directory view (tracked + local overlay).
#[derive(Debug, Clone)]
pub struct MergedFile {
    /// The actual file path on disk to read from
    pub source: PathBuf,
    /// Relative path within the directory
    pub rel_path: PathBuf,
    /// Whether this file comes from the local overlay
    pub is_local: bool,
}

/// Collect merged files from a source directory and optional local overlay,
/// with exclude filtering applied to the merged result.
pub fn collect_merged_files(
    source: &Path,
    local_dir: Option<&Path>,
    exclude: Option<&GlobSet>,
) -> Result<Vec<MergedFile>> {
    // Collect tracked files (no exclude yet -- we apply after merge)
    let mut tracked = HashMap::new();
    if source.exists() {
        for entry in WalkDir::new(source).min_depth(1) {
            let entry = entry?;
            if entry.file_type().is_dir() {
                continue;
            }
            let rel_path = entry.path().strip_prefix(source)?.to_path_buf();
            tracked.insert(rel_path, entry.path().to_path_buf());
        }
    }

    // Collect local overlay files
    let mut local_files = HashMap::new();
    if let Some(ld) = local_dir
        && ld.exists()
    {
        for entry in WalkDir::new(ld).min_depth(1) {
            let entry = entry?;
            if entry.file_type().is_dir() {
                continue;
            }
            let rel_path = entry.path().strip_prefix(ld)?.to_path_buf();
            local_files.insert(rel_path, entry.path().to_path_buf());
        }
    }

    // Merge: local overrides tracked
    let mut merged = Vec::new();
    let mut all_rel_paths: HashSet<PathBuf> = tracked.keys().cloned().collect();
    for k in local_files.keys() {
        all_rel_paths.insert(k.clone());
    }

    let mut sorted_paths: Vec<PathBuf> = all_rel_paths.into_iter().collect();
    sorted_paths.sort();

    for rel_path in sorted_paths {
        // Apply exclude to the merged result
        if let Some(gs) = exclude
            && gs.is_match(&rel_path)
        {
            continue;
        }

        if let Some(local_source) = local_files.get(&rel_path) {
            merged.push(MergedFile {
                source: local_source.clone(),
                rel_path,
                is_local: true,
            });
        } else if let Some(tracked_source) = tracked.get(&rel_path) {
            merged.push(MergedFile {
                source: tracked_source.clone(),
                rel_path,
                is_local: false,
            });
        }
    }

    Ok(merged)
}

/// Copy a source directory to target, with optional local overlay and exclude patterns.
fn copy_directory(
    source: &Path,
    target: &Path,
    local_dir: Option<&Path>,
    exclude: Option<&GlobSet>,
    immutable: bool,
    mode: Option<u32>,
    dry_run: bool,
) -> Result<()> {
    if !source.exists() {
        return Err(anyhow::anyhow!(
            "Source directory does not exist: {}",
            source.display()
        ));
    }

    prepare_exact_node(target, NodeKind::Directory, dry_run)?;
    if !dry_run {
        ensure_dir(target)?;
    }

    let merged = collect_merged_files(source, local_dir, exclude)?;

    for mf in &merged {
        let target_path = target.join(&mf.rel_path);

        if dry_run {
            let overlay_note = if mf.is_local { " (local)" } else { "" };
            log::info(&format!(
                "Would copy to {}{}",
                target_path.display(),
                overlay_note
            ));
            continue;
        }

        if let Some(relative_parent) = mf.rel_path.parent() {
            prepare_managed_parent_dirs(target, relative_parent, false)?;
        }
        copy_file(&mf.source, &target_path, mode, immutable, false)?;
        if immutable {
            set_immutable_flag(&target_path)?;
        }
    }

    Ok(())
}

/// Get the evaluated Order for an order
pub fn get_order(ctx: &Context, order_name: &str) -> Result<Order> {
    let order_dir = ctx.orders_dir.join(order_name);
    let ncl_path = order_dir.join("order.ncl");

    let evaluator = NickelEvaluator::new(&ctx.metadata);
    evaluator.evaluate(&ncl_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn test_atomic_replace_failure_preserves_existing_target() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("config");
        std::fs::write(&target, "original").unwrap();

        let error = atomic_replace_file(&target, None, None, false, |file| {
            file.write_all(b"partial")?;
            anyhow::bail!("injected write failure")
        })
        .unwrap_err();

        assert!(error.to_string().contains("injected write failure"));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        let leftovers: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".blend-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary replacement was not cleaned up"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_declared_mode_detects_and_clears_special_bits() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("tool");
        std::fs::write(&target, "tool").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o1755)).unwrap();

        assert_eq!(file_mode_mismatch(&target, 0o755).unwrap(), Some(0o1755));
        reconcile_regular_file_mode(&target, 0o755, false).unwrap();
        assert_eq!(
            target.metadata().unwrap().permissions().mode() & 0o7777,
            0o755
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_mode_reconciliation_refuses_symlink_without_touching_referent() {
        let temp = TempDir::new().unwrap();
        let backing = temp.path().join("backing");
        let target = temp.path().join("config");
        std::fs::write(&backing, "external").unwrap();
        std::fs::set_permissions(&backing, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink(&backing, &target).unwrap();

        let result = reconcile_regular_file_mode(&target, 0o600, false);
        #[cfg(target_os = "linux")]
        assert!(result.is_err(), "Linux must reject a symlink descriptor");
        #[cfg(not(target_os = "linux"))]
        let _ = result;
        assert!(target.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&backing).unwrap(), "external");
        assert_eq!(
            backing.metadata().unwrap().permissions().mode() & 0o7777,
            0o644
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_mode_reconciliation_repairs_inaccessible_file() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("config");
        std::fs::write(&target, "secret").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o000)).unwrap();
        #[cfg(target_os = "macos")]
        {
            assert!(
                std::process::Command::new("chflags")
                    .arg("uchg")
                    .arg(&target)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        reconcile_regular_file_mode(&target, 0o600, true).unwrap();
        assert_eq!(
            target.metadata().unwrap().permissions().mode() & 0o7777,
            0o600
        );
        #[cfg(target_os = "macos")]
        {
            use std::os::macos::fs::MetadataExt;
            let flags = target.metadata().unwrap().st_flags();
            assert!(
                std::process::Command::new("chflags")
                    .arg("nouchg")
                    .arg(&target)
                    .status()
                    .unwrap()
                    .success()
            );
            assert_ne!(
                flags & libc::UF_IMMUTABLE,
                0,
                "mode repair dropped immutable"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_unreadable_mode_repair_restores_immutable_without_reopening() {
        use std::os::macos::fs::MetadataExt;
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("config");
        std::fs::write(&target, "secret").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o200)).unwrap();
        immutable::set_no_follow(&target).unwrap();
        let result = reconcile_regular_file_mode(&target, 0, true);
        let metadata = target.metadata().unwrap();
        // Restore cleanup access before asserting, even on a regression.
        immutable::clear_no_follow(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        result.unwrap();
        assert_eq!(metadata.permissions().mode() & 0o7777, 0);
        assert_ne!(metadata.st_flags() & libc::UF_IMMUTABLE, 0);
    }

    #[cfg(unix)]
    #[test]
    fn test_atomic_replace_raced_symlink_does_not_mutate_referent() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("config");
        let backing = temp.path().join("backing");
        std::fs::write(&target, "original").unwrap();
        std::fs::write(&backing, "external").unwrap();
        std::fs::set_permissions(&backing, std::fs::Permissions::from_mode(0o644)).unwrap();

        atomic_replace_file(&target, Some(0o600), None, false, |file| {
            file.write_all(b"replacement")?;
            std::fs::remove_file(&target)?;
            std::os::unix::fs::symlink(&backing, &target)?;
            Ok(())
        })
        .unwrap();

        assert!(target.symlink_metadata().unwrap().file_type().is_file());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "replacement");
        assert_eq!(std::fs::read_to_string(&backing).unwrap(), "external");
        assert_eq!(
            backing.metadata().unwrap().permissions().mode() & 0o7777,
            0o644
        );
        let leftovers: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".blend-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn test_discover_orders() {
        let temp = TempDir::new().unwrap();
        let orders = temp.path();

        // Create order with order.ncl
        let order1 = orders.join("order1");
        std::fs::create_dir(&order1).unwrap();
        std::fs::write(order1.join("order.ncl"), "{}").unwrap();

        // Create order without order.ncl
        let order2 = orders.join("order2");
        std::fs::create_dir(&order2).unwrap();

        let orders = discover_orders(orders);
        assert!(orders.contains("order1"));
        assert!(!orders.contains("order2"));
    }

    #[test]
    fn test_build_glob_set_empty() {
        let result = build_glob_set(&[]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_build_glob_set_patterns() {
        let patterns = vec!["*.bak".to_string(), ".gitignore".to_string()];
        let gs = build_glob_set(&patterns).unwrap().unwrap();
        assert!(gs.is_match("file.bak"));
        assert!(gs.is_match(".gitignore"));
        assert!(!gs.is_match("file.txt"));
    }

    #[test]
    fn test_build_glob_set_nested_pattern() {
        let patterns = vec!["lib/tmp/**".to_string()];
        let gs = build_glob_set(&patterns).unwrap().unwrap();
        assert!(gs.is_match("lib/tmp/foo.txt"));
        assert!(gs.is_match("lib/tmp/sub/bar.txt"));
        assert!(!gs.is_match("lib/foo.txt"));
    }

    #[test]
    fn test_build_file_entry_preserves_explicit_format() {
        let temp = TempDir::new().unwrap();
        let entry = FileEntry {
            name: ".npmrc".to_string(),
            from_file: None,
            from_config: Some(serde_json::json!({
                "prefix": "~/.local/share/npm",
                "save-exact": "true",
            })),
            prefix: vec![],
            format: Some(Format::EqualsRecordLines),
            ignore: vec![],
            when: None,
            symlink: false,
            exclude: vec![],
            local: None,
            immutable: false,
            mode: None,
        };

        let result = build_file_entry(
            temp.path(),
            &entry,
            temp.path().join(".npmrc"),
            vec![],
            FileResolution::default(),
        )
        .unwrap();

        assert_eq!(result.format, Format::EqualsRecordLines);
        assert_eq!(result.content, "prefix=~/.local/share/npm\nsave-exact=true");
    }

    #[test]
    fn test_collect_merged_files_no_overlay() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir_all(source.join("sub")).unwrap();
        std::fs::write(source.join("a.txt"), "a").unwrap();
        std::fs::write(source.join("sub/b.txt"), "b").unwrap();

        let merged = collect_merged_files(&source, None, None).unwrap();
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().all(|m| !m.is_local));
    }

    #[test]
    fn test_collect_merged_files_with_exclude() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("keep.txt"), "keep").unwrap();
        std::fs::write(source.join("skip.bak"), "skip").unwrap();
        std::fs::write(source.join(".gitignore"), "ignore").unwrap();

        let patterns = vec!["*.bak".to_string(), ".gitignore".to_string()];
        let gs = build_glob_set(&patterns).unwrap();
        let merged = collect_merged_files(&source, None, gs.as_ref()).unwrap();

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].rel_path, std::path::PathBuf::from("keep.txt"));
    }

    #[test]
    fn test_collect_merged_files_with_local_overlay() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let local = temp.path().join("local");

        // Source files
        std::fs::create_dir_all(source.join("sub")).unwrap();
        std::fs::write(source.join("tracked.txt"), "tracked").unwrap();
        std::fs::write(source.join("sub/shared.txt"), "from-source").unwrap();

        // Local overlay: overrides sub/shared.txt and adds new.txt
        std::fs::create_dir_all(local.join("sub")).unwrap();
        std::fs::write(local.join("sub/shared.txt"), "from-local").unwrap();
        std::fs::write(local.join("new.txt"), "new-local").unwrap();

        let merged = collect_merged_files(&source, Some(&local), None).unwrap();

        assert_eq!(merged.len(), 3);

        // new.txt (local)
        let new = merged
            .iter()
            .find(|m| m.rel_path.as_path() == Path::new("new.txt"))
            .unwrap();
        assert!(new.is_local);

        // sub/shared.txt (local override)
        let shared = merged
            .iter()
            .find(|m| m.rel_path.as_path() == Path::new("sub/shared.txt"))
            .unwrap();
        assert!(shared.is_local);
        assert_eq!(
            std::fs::read_to_string(&shared.source).unwrap(),
            "from-local"
        );

        // tracked.txt (from source)
        let tracked = merged
            .iter()
            .find(|m| m.rel_path.as_path() == Path::new("tracked.txt"))
            .unwrap();
        assert!(!tracked.is_local);
    }

    #[test]
    fn test_collect_merged_files_exclude_applies_to_local() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let local = temp.path().join("local");

        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("a.txt"), "a").unwrap();

        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("skip.bak"), "skip").unwrap();
        std::fs::write(local.join("keep.txt"), "keep").unwrap();

        let patterns = vec!["*.bak".to_string()];
        let gs = build_glob_set(&patterns).unwrap();
        let merged = collect_merged_files(&source, Some(&local), gs.as_ref()).unwrap();

        // Should have a.txt and keep.txt, but not skip.bak
        assert_eq!(merged.len(), 2);
        assert!(
            !merged
                .iter()
                .any(|m| m.rel_path.as_path() == Path::new("skip.bak"))
        );
    }

    #[test]
    fn test_copy_directory_with_exclude() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");

        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("keep.txt"), "keep").unwrap();
        std::fs::write(source.join("skip.bak"), "skip").unwrap();

        let patterns = vec!["*.bak".to_string()];
        let gs = build_glob_set(&patterns).unwrap();

        copy_directory(&source, &target, None, gs.as_ref(), false, None, false).unwrap();

        assert!(target.join("keep.txt").exists());
        assert!(!target.join("skip.bak").exists());
    }

    #[test]
    fn test_copy_directory_with_local_overlay() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let local = temp.path().join("local");
        let target = temp.path().join("target");

        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("tracked.txt"), "from-source").unwrap();
        std::fs::write(source.join("shared.txt"), "source-version").unwrap();

        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("shared.txt"), "local-version").unwrap();
        std::fs::write(local.join("extra.txt"), "local-only").unwrap();

        copy_directory(&source, &target, Some(&local), None, false, None, false).unwrap();

        assert_eq!(
            std::fs::read_to_string(target.join("tracked.txt")).unwrap(),
            "from-source"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("shared.txt")).unwrap(),
            "local-version"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("extra.txt")).unwrap(),
            "local-only"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_write_result_preserves_symlinked_ancestor() {
        let temp = TempDir::new().unwrap();
        let backing = temp.path().join("backing");
        let linked_parent = temp.path().join("linked-parent");
        std::fs::create_dir_all(&backing).unwrap();
        std::os::unix::fs::symlink(&backing, &linked_parent).unwrap();

        let result = BuildResult {
            target: linked_parent.join("config.txt"),
            content: "new content\n".to_string(),
            is_plaintext: false,
            source_path: None,
            name: "config.txt".to_string(),
            format: Format::Plaintext,
            ignore_keys: vec![],
            is_symlink: false,
            canonical_source: None,
            exclude_patterns: vec![],
            local_dir: None,
            immutable: false,
            mode: None,
            automatic_source_paths: HashSet::new(),
            manual_resolution_paths: HashSet::new(),
            resource_disposition: ResourceDisposition::Present,
        };

        write_result(&result, false).unwrap();
        assert!(
            linked_parent
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(backing.join("config.txt")).unwrap(),
            "new content\n"
        );
    }

    #[test]
    fn test_write_result_replaces_exact_directory_with_file() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("config.txt");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("old.txt"), "old").unwrap();

        let result = BuildResult {
            target: target.clone(),
            content: "new content\n".to_string(),
            is_plaintext: false,
            source_path: None,
            name: "config.txt".to_string(),
            format: Format::Plaintext,
            ignore_keys: vec![],
            is_symlink: false,
            canonical_source: None,
            exclude_patterns: vec![],
            local_dir: None,
            immutable: false,
            mode: None,
            automatic_source_paths: HashSet::new(),
            manual_resolution_paths: HashSet::new(),
            resource_disposition: ResourceDisposition::Present,
        };

        write_result(&result, false).unwrap();
        assert_eq!(node_kind(&target).unwrap(), Some(NodeKind::File));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new content\n");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn test_write_result_clears_existing_immutable_even_when_desired_false() {
        use crate::immutable::{TestImmutableEventKind, take_test_events};

        let _guard = env_lock().lock().unwrap();
        take_test_events();
        let temp = TempDir::new().unwrap();

        let target = temp.path().join("target.txt");
        std::fs::write(&target, "old").unwrap();

        let result = BuildResult {
            target: target.clone(),
            content: "new".to_string(),
            is_plaintext: false,
            source_path: None,
            name: "target.txt".to_string(),
            format: Format::Plaintext,
            ignore_keys: vec![],
            is_symlink: false,
            canonical_source: None,
            exclude_patterns: vec![],
            local_dir: None,
            immutable: false,
            mode: None,
            automatic_source_paths: HashSet::new(),
            manual_resolution_paths: HashSet::new(),
            resource_disposition: ResourceDisposition::Present,
        };

        write_result(&result, false).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");

        let events = take_test_events();
        assert!(
            events
                .iter()
                .any(|(kind, path)| *kind == TestImmutableEventKind::Clear && path == &target),
            "expected clear immutable event for target, got: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|(kind, path)| *kind == TestImmutableEventKind::Set && path == &target),
            "desired immutable=false should not set immutable, got: {events:?}"
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn test_write_result_directory_clears_immutable_only_for_managed_files() {
        use crate::immutable::{TestImmutableEventKind, take_test_events};

        let _guard = env_lock().lock().unwrap();
        take_test_events();
        let temp = TempDir::new().unwrap();

        let source = temp.path().join("source");
        let target = temp.path().join("target");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(source.join("managed.txt"), "new").unwrap();
        std::fs::write(target.join("managed.txt"), "old").unwrap();
        std::fs::write(target.join("target-only.txt"), "cache").unwrap();

        let result = BuildResult {
            target: target.clone(),
            content: String::new(),
            is_plaintext: true,
            source_path: Some(source),
            name: "target".to_string(),
            format: Format::Plaintext,
            ignore_keys: vec![],
            is_symlink: false,
            canonical_source: None,
            exclude_patterns: vec![],
            local_dir: None,
            immutable: false,
            mode: None,
            automatic_source_paths: HashSet::new(),
            manual_resolution_paths: HashSet::new(),
            resource_disposition: ResourceDisposition::Present,
        };

        write_result(&result, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("managed.txt")).unwrap(),
            "new"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("target-only.txt")).unwrap(),
            "cache"
        );

        let events = take_test_events();
        assert!(
            events
                .iter()
                .any(|(kind, path)| *kind == TestImmutableEventKind::Clear
                    && path == &target.join("managed.txt")),
            "managed file should be prepared for overwrite, got: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|(_, path)| path == &target.join("target-only.txt")),
            "target-only files must not be touched, got: {events:?}"
        );
    }
}
