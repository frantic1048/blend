//! Explicit resources have node properties, not semantic file content.
use crate::compose::{BuildResult, write_result};
use crate::context::Context;
use crate::fs_node::{NodeKind, node_kind};
use crate::nickel::{NickelEvaluator, ResourceKind};
use crate::output::log;
use crate::sync::{Prompter, SyncAction, SyncMode};
use anyhow::{Context as _, Result, bail};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

struct Owner {
    label: String,
    order: String,
    kind: NodeKind,
}
struct DirectoryMode {
    order: String,
    target: PathBuf,
    mode: Option<u32>,
}
struct ValidatedSource {
    source: PathBuf,
    local: Option<PathBuf>,
    exclude: Vec<String>,
    manifest: crate::compose::SourceManifest,
}

pub struct Ownership {
    owners: HashMap<PathBuf, Owner>,
    directories: Vec<DirectoryMode>,
    manifests: HashMap<PathBuf, ValidatedSource>,
}

/// Reject duplicate or incompatible active ownership before any Target mutation.
/// Existence-only directories own no children; content trees own their emitted nodes.
pub fn validate_ownership(ctx: &Context) -> Result<Ownership> {
    let mut owners = HashMap::<PathBuf, Owner>::new();
    let mut directories = Vec::new();
    let mut manifests = HashMap::new();
    let mut orders: Vec<_> = crate::compose::discover_orders(&ctx.orders_dir)
        .into_iter()
        .collect();
    orders.sort();
    for name in orders {
        crate::interrupt::check()?;
        if ctx.verbose {
            log::info(&format!("Validating ownership for {name}"));
        }
        // Ownership is an inventory of evaluable Orders. An invalid unrelated
        // Order must not prevent selecting a healthy one; check/build reports
        // evaluation errors for each selected Order through its normal loop.
        let evaluated = NickelEvaluator::new(&ctx.metadata)
            .evaluate(&ctx.orders_dir.join(&name).join("order.ncl"));
        crate::interrupt::check()?;
        let Ok(order) = evaluated else {
            continue;
        };
        if !order.should_apply(&ctx.metadata.os, &ctx.metadata.arch, &ctx.metadata.hostname) {
            continue;
        }
        for entry in &order.blend.files {
            crate::interrupt::check()?;
            if !entry.should_apply(&ctx.metadata.os, &ctx.metadata.arch, &ctx.metadata.hostname) {
                continue;
            }
            let base = ctx.orders_dir.join(&name);
            let source = entry.from_file.as_ref().map(|source| base.join(source));
            let kind = match entry.effective_kind() {
                ResourceKind::Symlink => NodeKind::Symlink,
                ResourceKind::Directory => NodeKind::Directory,
                ResourceKind::Content if source.as_ref().is_some_and(|path| path.is_dir()) => {
                    NodeKind::Directory
                }
                ResourceKind::Content => NodeKind::File,
            };
            for target in entry.target_paths(order.global_prefix()) {
                let target = lexical_path(&ctx.expand_path(&target));
                if entry.effective_kind() == ResourceKind::Directory {
                    directories.push(DirectoryMode {
                        order: name.clone(),
                        target: target.clone(),
                        mode: entry.parsed_mode()?,
                    });
                }
                let mut paths = HashMap::from([(target.clone(), kind)]);
                if entry.effective_kind() == ResourceKind::Content && kind == NodeKind::Directory {
                    let exclude = crate::compose::build_glob_set(&entry.exclude)?;
                    let local = entry.local.as_ref().map(|s| base.join(s));
                    let files = crate::compose::collect_merged_files(
                        source.as_ref().unwrap(),
                        local.as_deref(),
                        exclude.as_ref(),
                    )?;
                    manifests.insert(
                        target.clone(),
                        ValidatedSource {
                            source: source.clone().unwrap(),
                            local: local.clone(),
                            exclude: entry.exclude.clone(),
                            manifest: crate::compose::SourceManifest::Tree(files.clone()),
                        },
                    );
                    for file in files {
                        let path = lexical_path(&target.join(&file.rel_path));
                        // A content tree actively creates/reconciles its interior
                        // directories; unlike an entry's unowned ancestor paths.
                        let mut parent = path.parent();
                        while let Some(path) = parent {
                            if path == target {
                                break;
                            }
                            if !path.starts_with(&target) {
                                bail!("managed child escapes its Target");
                            }
                            if paths
                                .insert(path.to_owned(), NodeKind::Directory)
                                .is_some_and(|kind| kind != NodeKind::Directory)
                            {
                                bail!("incompatible content-tree paths at {}", path.display());
                            }
                            parent = path.parent();
                        }
                        if paths.insert(path.clone(), NodeKind::File).is_some() {
                            bail!("incompatible content-tree paths at {}", path.display());
                        }
                    }
                }
                if entry.effective_kind() == ResourceKind::Content
                    && source.is_some()
                    && kind == NodeKind::File
                {
                    manifests.insert(
                        target.clone(),
                        ValidatedSource {
                            source: source.clone().unwrap(),
                            local: entry.local.as_ref().map(|path| base.join(path)),
                            exclude: entry.exclude.clone(),
                            manifest: crate::compose::SourceManifest::File,
                        },
                    );
                }
                for (path, kind) in paths {
                    let owner = Owner {
                        label: format!("{name}:{}", entry.name),
                        order: name.clone(),
                        kind,
                    };
                    if let Some(previous) = owners.get(&path) {
                        bail!(
                            "Duplicate Target {} owned by {} and {}",
                            path.display(),
                            previous.label,
                            owner.label
                        );
                    }
                    owners.insert(path, owner);
                }
            }
        }
    }
    for (path, owner) in &owners {
        for ancestor in path.ancestors().skip(1) {
            if let Some(parent) = owners.get(ancestor)
                && parent.kind != NodeKind::Directory
            {
                bail!(
                    "Incompatible Target ownership: {} ({}) is below managed {} {} ({})",
                    path.display(),
                    owner.label,
                    parent.kind,
                    ancestor.display(),
                    parent.label
                );
            }
        }
    }
    validate_physical_ownership(&owners)?;
    for (path, owner) in &owners {
        if owner.kind == NodeKind::Directory
            && !directories
                .iter()
                .any(|directory| &directory.target == path)
        {
            directories.push(DirectoryMode {
                order: owner.order.clone(),
                target: path.clone(),
                mode: None,
            });
        }
    }
    Ok(Ownership {
        owners,
        directories,
        manifests,
    })
}

// Resolve only unowned infrastructure. A managed node remains an exact-node
// boundary even when its current Target happens to be a symlink.
fn physical_key(owners: &HashMap<PathBuf, Owner>, path: &Path) -> Result<PathBuf> {
    let boundary = path
        .ancestors()
        .filter(|ancestor| owners.contains_key(*ancestor))
        .last()
        .unwrap_or(path);
    let mut prefix = boundary
        .parent()
        .context("Target has no parent")?
        .to_owned();
    let mut suffix = Vec::new();
    let resolved = loop {
        match std::fs::canonicalize(&prefix) {
            Ok(path) => break path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                suffix.push(
                    prefix
                        .file_name()
                        .context("Cannot resolve Target prefix")?
                        .to_owned(),
                );
                if !prefix.pop() {
                    return Err(error.into());
                }
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Cannot validate Target aliases at {}", prefix.display())
                });
            }
        }
    };
    let case_sensitive = filesystem_case_sensitive(&resolved)?;
    let mut physical = resolved;
    for component in suffix.iter().rev() {
        physical.push(component);
    }
    physical.push(path.strip_prefix(boundary.parent().unwrap())?);
    if case_sensitive {
        Ok(physical)
    } else {
        Ok(PathBuf::from(physical.to_string_lossy().to_lowercase()))
    }
}

// Query the existing filesystem without creating probe files during read-only
// commands. In particular, APFS volumes can be either case-sensitive or not.
#[cfg(target_os = "macos")]
fn filesystem_case_sensitive(path: &Path) -> Result<bool> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: path is a valid NUL-terminated string retained for the call.
    match unsafe { libc::pathconf(path.as_ptr(), libc::_PC_CASE_SENSITIVE) } {
        0 => Ok(false),
        1 => Ok(true),
        _ => bail!("Cannot determine Target filesystem case sensitivity"),
    }
}

#[cfg(not(target_os = "macos"))]
fn filesystem_case_sensitive(_path: &Path) -> Result<bool> {
    Ok(true)
}

fn validate_physical_ownership(owners: &HashMap<PathBuf, Owner>) -> Result<()> {
    let mut physical = HashMap::<PathBuf, &PathBuf>::new();
    for path in owners.keys() {
        let key = physical_key(owners, path)?;
        if let Some(other) = physical.insert(key, path) {
            bail!(
                "Aliased or case-ambiguous Targets {} and {} have duplicate ownership",
                other.display(),
                path.display()
            );
        }
    }
    for (key, path) in &physical {
        for ancestor in key.ancestors().skip(1) {
            if let Some(parent) = physical.get(ancestor)
                && (owners[*parent].kind != NodeKind::Directory || !path.starts_with(*parent))
            {
                bail!(
                    "Aliased managed ancestor {} conflicts with Target {}; use consistent, non-overlapping Target paths",
                    parent.display(),
                    path.display()
                );
            }
        }
    }
    Ok(())
}

/// Temporary owner access for descendant maintenance, restored even on an error
/// or user abort. Unselected ancestor Orders retain their pre-sync permissions.
pub struct DirectoryAccess {
    entries: Vec<(PathBuf, u32)>,
    dry_run: bool,
    initial_mode_drift: HashSet<PathBuf>,
}
impl DirectoryAccess {
    pub fn mode_changed(&self, target: &Path) -> bool {
        self.initial_mode_drift.contains(&lexical_path(target))
    }

    pub fn set_final_mode(&mut self, target: &Path, mode: Option<u32>) {
        let Some(mode) = mode else { return };
        if let Some((_, restore)) = self.entries.iter_mut().find(|(path, _)| path == target) {
            *restore = mode;
        } else {
            self.entries.push((target.to_owned(), mode));
        }
    }

    pub fn finish(&mut self) -> Result<()> {
        self.entries
            .sort_by_key(|(path, _)| path.components().count());
        let mut error = None;
        while let Some((path, mode)) = self.entries.pop() {
            if !self.dry_run
                && let Err(err) = crate::compose::reconcile_directory_mode(&path, Some(mode))
            {
                log::error(&format!(
                    "Could not restore directory mode on {}: {err:#}",
                    path.display()
                ));
                error = Some(err);
            }
        }
        if let Some(error) = error {
            return Err(error);
        }
        Ok(())
    }
}
impl Drop for DirectoryAccess {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

pub fn prepare_directory_access(
    ctx: &Context,
    ownership: &mut Ownership,
    selected: &[String],
) -> Result<DirectoryAccess> {
    ownership
        .directories
        .sort_by_key(|directory| directory.target.components().count());
    let mut access = DirectoryAccess {
        entries: Vec::new(),
        dry_run: ctx.dry_run,
        initial_mode_drift: HashSet::new(),
    };
    for directory in &ownership.directories {
        crate::interrupt::check()?;
        let has_descendants = ownership.owners.iter().any(|(path, owner)| {
            selected.contains(&owner.order)
                && path != &directory.target
                && path.starts_with(&directory.target)
        });
        if !has_descendants && !selected.contains(&directory.order) {
            continue;
        }
        // Do not inspect or chmod children through an unexpected managed link.
        // Unowned ancestor links remain ordinary path-resolution infrastructure.
        let mut managed_ancestors: Vec<_> = directory
            .target
            .ancestors()
            .skip(1)
            .filter(|path| ownership.owners.contains_key(*path))
            .collect();
        managed_ancestors.reverse();
        let mut blocked = false;
        for ancestor in managed_ancestors {
            if node_kind(ancestor)? != Some(NodeKind::Directory) {
                blocked = true;
                break;
            }
        }
        if blocked || node_kind(&directory.target)? != Some(NodeKind::Directory) {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let actual = directory.target.symlink_metadata()?.permissions().mode() & 0o7777;
            if selected.contains(&directory.order)
                && directory.mode.is_some_and(|mode| mode != actual)
            {
                access.initial_mode_drift.insert(directory.target.clone());
            }
            if !has_descendants {
                continue;
            }
            let temporary = actual | 0o700;
            let restore = if selected.contains(&directory.order) {
                directory.mode.unwrap_or(actual)
            } else {
                actual
            };
            access.entries.push((directory.target.clone(), restore));
            if actual != temporary {
                if ctx.dry_run {
                    log::info(&format!(
                        "Would temporarily allow owner access to {} and restore mode {:04o}",
                        directory.target.display(),
                        restore
                    ));
                } else {
                    crate::compose::reconcile_directory_mode(&directory.target, Some(temporary))?;
                }
            }
        }
    }
    Ok(access)
}

/// A declared ancestor belongs to Blend's node model, unlike an unowned parent
/// symlink used as path infrastructure. Reconcile a mismatched owner first.
pub fn assert_managed_ancestors(ownership: &Ownership, target: &Path) -> Result<()> {
    let target = lexical_path(target);
    let mut ancestors: Vec<_> = target
        .ancestors()
        .skip(1)
        .filter(|path| ownership.owners.contains_key(*path))
        .collect();
    ancestors.reverse();
    for ancestor in ancestors {
        match node_kind(ancestor)? {
            Some(NodeKind::Directory) => {}
            None => return Ok(()),
            Some(actual) => bail!(
                "Managed ancestor {} is a {actual}; reconcile {} before syncing its descendants",
                ancestor.display(),
                ownership.owners[ancestor].label
            ),
        }
    }
    Ok(())
}

pub fn assert_reconciliation_ancestors(
    ownership: &Ownership,
    target: &Path,
    planned: &HashSet<PathBuf>,
) -> Result<()> {
    let target = lexical_path(target);
    let mut ancestors: Vec<_> = target
        .ancestors()
        .skip(1)
        .filter(|path| ownership.owners.contains_key(*path))
        .collect();
    ancestors.reverse();
    for ancestor in ancestors {
        if planned.contains(ancestor) {
            continue;
        }
        if node_kind(ancestor)? != Some(NodeKind::Directory) {
            bail!(
                "Managed ancestor {} is missing or has the wrong kind; reconcile {} before syncing its descendants",
                ancestor.display(),
                ownership.owners[ancestor].label
            );
        }
    }
    Ok(())
}

pub fn record_planned_directories(result: &BuildResult, planned: &mut HashSet<PathBuf>) {
    if crate::commands::helpers::expected_node_kind(result) != NodeKind::Directory {
        return;
    }
    planned.insert(lexical_path(&result.target));
    if let Some(crate::compose::SourceManifest::Tree(files)) = &result.validated_source {
        for file in files {
            let mut parent = file.rel_path.parent();
            while let Some(relative) = parent {
                planned.insert(lexical_path(&result.target.join(relative)));
                parent = relative.parent();
            }
        }
    }
}

pub fn bind_source_manifest(ownership: &Ownership, result: &mut BuildResult) -> Result<()> {
    if result.kind == ResourceKind::Content && result.source_path.is_some() {
        let manifest = ownership
            .manifests
            .get(&lexical_path(&result.target))
            .context("Source changed since ownership validation; retry sync")?;
        if result.source_path.as_ref() != Some(&manifest.source)
            || result.local_dir != manifest.local
            || result.exclude_patterns != manifest.exclude
        {
            bail!("Source declaration changed since ownership validation; retry sync");
        }
        result.validated_source = Some(manifest.manifest.clone());
    }
    Ok(())
}

pub(crate) fn lexical_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .expect("working directory is accessible")
            .join(path)
    };
    let mut result = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::CurDir => {}
            _ => result.push(component),
        }
    }
    result
}

/// Rewrite only resource fields of a concrete declaration. Existing values are
/// replaced in place, retaining surrounding comments. Removed fields leave their
/// comments intact. Computed entry selection is deliberately unsupported.
struct Adoption {
    original: String,
    updated: String,
    resource: BuildResult,
}

fn adopted_declaration(
    ctx: &Context,
    order: &str,
    index: usize,
    result: &BuildResult,
) -> Result<Adoption> {
    let path = ctx.orders_dir.join(order).join("order.ncl");
    let source = std::fs::read_to_string(&path)?;
    let record = crate::nickel::structure_map::file_entry_record(&source, index)?;
    let evaluator = NickelEvaluator::new(&ctx.metadata);
    let old_order = evaluator.evaluate_source(&path, &source)?;
    let old_entry = old_order
        .blend
        .files
        .get(index)
        .context("entry is absent")?;
    if old_entry.immutable
        || !old_entry.ignore.is_empty()
        || !result.ignore_keys.is_empty()
        || old_entry.local.is_some()
        || !old_entry.exclude.is_empty()
    {
        bail!(
            "kind adoption would discard immutable, ignore, local, or exclude settings; edit order.ncl manually"
        );
    }
    let comment_ranges = declaration_comment_ranges(&source)?;
    let mut desired = HashMap::<&str, String>::new();
    // Preserve the effective name when removing the from_file default.
    if !record
        .fields
        .iter()
        .any(|field| field.key_path.segments() == ["name"])
    {
        desired.insert("name", literal(&result.name));
    }
    match node_kind(&result.target)? {
        Some(NodeKind::Symlink) => {
            let link = std::fs::read_link(&result.target)?;
            let text = link
                .to_str()
                .context("link destination is not UTF-8; edit order.ncl manually")?;
            desired.insert("kind", literal("symlink"));
            desired.insert("link_to", literal(text));
        }
        Some(NodeKind::File) => {
            desired.insert("kind", literal("content"));
            desired.insert("format", literal("plaintext"));
            if let Some(mode) = result.mode {
                desired.insert("mode", literal(&format!("{mode:04o}")));
            }
            // Never follow a raced-in symlink while importing Target content.
            #[cfg(unix)]
            {
                use std::io::Read;
                use std::os::unix::fs::OpenOptionsExt;
                let mut file = std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(&result.target)?;
                if !file.metadata()?.is_file() {
                    bail!("Target changed type during adoption");
                }
                let mut content = String::new();
                file.read_to_string(&mut content)
                    .context("Target content is not UTF-8; edit order.ncl manually")?;
                desired.insert("from_config", literal(&content));
            }
            #[cfg(not(unix))]
            bail!("resource adoption requires Unix");
        }
        Some(NodeKind::Directory) => bail!(
            "Target is a directory: import its contents as a content resource with from_file, or explicitly declare kind = \"directory\"; edit order.ncl manually"
        ),
        _ => bail!("unsupported Target node; edit order.ncl manually"),
    }
    let resource_fields = [
        "kind",
        "link_to",
        "from_file",
        "from_config",
        "symlink",
        "local",
        "exclude",
        "mode",
        "immutable",
        "format",
        "ignore",
    ];
    let mut edits = Vec::new();
    for field in &record.fields {
        let key = field.key_path.segments()[0].as_str();
        if let Some(value) = desired.remove(key) {
            edits.push((field.value_range.clone(), value));
        } else if resource_fields.contains(&key) {
            // Preserve comments, including comments inside removed expressions.
            if comment_ranges.iter().any(|comment| {
                comment.start < field.full_range.end && comment.end > field.full_range.start
            }) {
                bail!("resource field '{key}' contains comments; edit order.ncl manually");
            }
            edits.push((field.full_range.clone(), String::new()));
        }
    }
    let mut additions: Vec<_> = desired.into_iter().collect();
    additions.sort_by_key(|(key, _)| *key);
    let mut insert = String::new();
    if let Some(last) = record.fields.last()
        && !last.has_trailing_comma
        && !edits
            .iter()
            .any(|(range, value)| range == &last.full_range && value.is_empty())
    {
        edits.push((last.full_range.end..last.full_range.end, ",".to_string()));
    }
    for (key, value) in additions {
        insert.push_str(&format!(
            "\n{}{} = {},",
            " ".repeat(record.field_indent.max(2)),
            key,
            value
        ));
    }
    insert.push('\n');
    edits.push((record.close_brace..record.close_brace, insert));
    edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    let mut output = source.clone();
    for (range, value) in edits {
        output.replace_range(range, &value);
    }
    // Evaluate in memory before writing. An annotation or surrounding expression
    // may transform a literal record, so CST location alone cannot prove adoption.
    let output = crate::nickel::format_source(&output)?;
    let new_order = evaluator
        .evaluate_source(&path, &output)
        .context("rewritten declaration does not evaluate safely; edit order.ncl manually")?;
    if old_order.blend.files.len() != new_order.blend.files.len() {
        bail!("rewrite changed the entry collection; edit order.ncl manually");
    }
    for (i, (old, new)) in old_order
        .blend
        .files
        .iter()
        .zip(&new_order.blend.files)
        .enumerate()
    {
        if i != index && old != new {
            bail!("rewrite affects another entry; edit order.ncl manually");
        }
    }
    let new_entry = new_order
        .blend
        .files
        .get(index)
        .context("rewritten entry is absent")?;
    let old_entry = &old_order.blend.files[index];
    if old_entry.target_paths(old_order.global_prefix()).len() != 1 {
        bail!(
            "resource entry has multiple Targets; edit order.ncl manually to resolve them together"
        );
    }
    if new_entry.target_paths(new_order.global_prefix())
        != old_entry.target_paths(old_order.global_prefix())
        || new_entry.when != old_entry.when
    {
        bail!("rewrite changes Target selection; edit order.ncl manually");
    }
    let built = crate::compose::build_file_entry_pub(
        ctx,
        path.parent().unwrap(),
        index,
        new_order.entry_has_resolution_semantics(index),
        new_entry,
        result.target.clone(),
        vec![],
    )?;
    if crate::commands::helpers::compute_content_diff_for_result(&built)?.has_changes {
        bail!(
            "rewritten declaration does not describe the observed Target; edit order.ncl manually"
        );
    }
    Ok(Adoption {
        original: source,
        updated: output,
        resource: built,
    })
}

/// Inspect actual CST comments, so `#` inside a string is ordinary data.
fn declaration_comment_ranges(source: &str) -> Result<Vec<std::ops::Range<usize>>> {
    fn collect(node: tree_sitter::Node<'_>, comments: &mut Vec<std::ops::Range<usize>>) {
        if node.kind() == "comment" {
            comments.push(node.byte_range());
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect(child, comments);
        }
    }
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_nickel::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .context("Could not parse Order")?;
    let mut comments = Vec::new();
    collect(tree.root_node(), &mut comments);
    Ok(comments)
}

/// Cooperating adoptions lock the original inode. Recheck its identity and
/// bytes immediately before an atomic rename, so stale proposals never truncate
/// or overwrite a newer Order. External editors must also cooperate for a
/// stronger compare-and-swap guarantee than the filesystem exposes.
fn atomic_source_edit(
    path: &Path,
    original: &str,
    updated: &str,
    validate_target: impl FnOnce() -> Result<()>,
) -> Result<()> {
    use std::io::{Read, Seek, Write};
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut source = options.open(path)?;
    if !source.metadata()?.is_file() {
        bail!("Source is not a regular file; edit order.ncl manually");
    }
    source
        .lock()
        .context("Could not lock Source for adoption")?;
    let metadata = source.metadata()?;
    let mut staged = tempfile::Builder::new()
        .prefix(".blend-order-")
        .tempfile_in(path.parent().context("Order has no parent")?)?;
    staged.as_file().set_permissions(metadata.permissions())?;
    staged.write_all(updated.as_bytes())?;
    staged.flush()?;
    staged.as_file().sync_all()?;
    validate_target()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let current = path.symlink_metadata()?;
        if current.dev() != metadata.dev() || current.ino() != metadata.ino() {
            bail!("Source changed during reconciliation; retry sync");
        }
    }
    source.rewind()?;
    let mut current = String::new();
    source.read_to_string(&mut current)?;
    if current != original {
        bail!("Source changed during reconciliation; retry sync");
    }
    staged.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn validate_adoption_target(resource: &BuildResult) -> Result<()> {
    let matches = if resource.kind == ResourceKind::Symlink {
        std::fs::read_link(&resource.target).is_ok_and(|link| {
            resource
                .canonical_source
                .as_ref()
                .is_some_and(|expected| expected.as_os_str() == link.as_os_str())
        })
    } else {
        use std::io::Read;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = options
            .open(&resource.target)
            .context("Target changed during reconciliation; retry sync")?;
        if !file.metadata()?.is_file() {
            bail!("Target changed during reconciliation; retry sync");
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        bytes == resource.content.as_bytes()
    };
    if !matches {
        bail!("Target changed during reconciliation; retry sync");
    }
    Ok(())
}

fn literal(text: &str) -> String {
    crate::nickel::ast_utils::json_to_nickel(&serde_json::Value::String(text.to_owned()), 0)
}

/// Handle exact-node reconciliation before any content reads. Returns the
/// chosen action so the caller can maintain its normal summary/abort behavior.
pub fn sync_resource(
    ctx: &Context,
    order: &str,
    index: usize,
    result: &BuildResult,
    mode: SyncMode,
    no_rewrite: bool,
    prompter: &dyn Prompter,
) -> Result<SyncAction> {
    let target_kind = node_kind(&result.target)?;
    let diff = crate::commands::helpers::compute_diff_for_result(result)?;
    if !diff.has_changes {
        if !ctx.dry_run && result.kind != ResourceKind::Content {
            ctx.state.forget_content(order, &result.target)?;
        }
        return Ok(SyncAction::Skip);
    }
    // Directory modes are finalized after descendant work by cmd_sync.
    if result.kind == ResourceKind::Directory && target_kind == Some(NodeKind::Directory) {
        if !ctx.dry_run {
            ctx.state.forget_content(order, &result.target)?;
        }
        return Ok(SyncAction::Skip);
    }
    if !result.manual_resolution_paths.is_empty() {
        bail!(
            "Cannot reconcile resource kinds while a resolver is unresolved; edit order.ncl manually"
        );
    }
    if result
        .automatic_source_paths
        .contains(&crate::nickel::key_path::KeyPath::root())
    {
        write_result(result, ctx.dry_run)?;
        return Ok(SyncAction::ApplySourceToTarget);
    }
    let proposed = if matches!(mode, SyncMode::ApplyTargetToSourceAll)
        && target_kind != Some(crate::commands::helpers::expected_node_kind(result))
    {
        Err(anyhow::anyhow!(
            "target structure does not match Source; forced Target -> Source cannot change resource kind; reconcile interactively or edit order.ncl manually"
        ))
    } else if !result.automatic_source_paths.is_empty() {
        Err(anyhow::anyhow!(
            "kind adoption would discard declared resolution authority; edit order.ncl manually"
        ))
    } else if matches!(mode, SyncMode::ApplySourceToTargetAll) {
        Err(anyhow::anyhow!("Target adoption not requested"))
    } else if no_rewrite {
        Err(anyhow::anyhow!(
            "--no-rewrite is set; edit order.ncl manually"
        ))
    } else {
        adopted_declaration(ctx, order, index, result)
    };
    let can_pull = proposed.is_ok();
    crate::sync::display_conflict(order, &result.name, &result.target, &diff, &ctx.home_dir);
    let action = match mode {
        SyncMode::ApplySourceToTargetAll => SyncAction::ApplySourceToTarget,
        SyncMode::ApplyTargetToSourceAll => {
            if !can_pull {
                bail!(
                    "Cannot apply Target -> Source for {order}:{}: {}",
                    result.name,
                    proposed.err().unwrap()
                );
            }
            SyncAction::ApplyTargetToSource
        }
        SyncMode::Interactive if ctx.dry_run => {
            log::info("[dry-run] would prompt for resource reconciliation");
            SyncAction::Skip
        }
        SyncMode::Interactive => {
            if let Err(error) = &proposed {
                log::warn(&format!("Target adoption unavailable: {error}"));
            }
            prompter.ask_sync_action(order, &result.name, &result.target, &diff, can_pull, None)
        }
    };
    match action {
        SyncAction::ApplySourceToTarget => {
            let mut deployment = result.clone();
            if deployment.kind == ResourceKind::Directory {
                deployment.mode = None;
            }
            write_result(&deployment, ctx.dry_run)?;
        }
        SyncAction::ApplyTargetToSource => {
            let adoption = proposed?;
            let path = ctx.orders_dir.join(order).join("order.ncl");
            if ctx.dry_run {
                log::info(&format!(
                    "[dry-run] would adopt Target resource into {order}:{}",
                    result.name
                ));
            } else {
                atomic_source_edit(&path, &adoption.original, &adoption.updated, || {
                    validate_adoption_target(&adoption.resource)
                })?;
                if adoption.resource.kind == ResourceKind::Content {
                    crate::compose::reconcile_file_mode(&adoption.resource, false)?;
                }
            }
        }
        _ => {}
    }
    if !ctx.dry_run
        && matches!(
            action,
            SyncAction::ApplySourceToTarget | SyncAction::ApplyTargetToSource
        )
    {
        ctx.state.forget_content(order, &result.target)?;
    }
    Ok(action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use tempfile::TempDir;

    fn adoption_fixture(entry: &str) -> (TempDir, Context, BuildResult) {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("source");
        let home = temp.path().join("home");
        let order = root.join("orders/test");
        std::fs::create_dir_all(order.join("tree")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(order.join("tree/file"), "source").unwrap();
        std::fs::write(
            order.join("order.ncl"),
            format!(r#"{{blend={{prefix=["~/"],files=[{entry}]}}}}"#),
        )
        .unwrap();
        let cli = crate::cli::Cli::parse_from([
            "blend",
            "--blend-dir",
            root.to_str().unwrap(),
            "--home",
            home.to_str().unwrap(),
        ]);
        let ctx = Context::new(&cli).unwrap();
        let result = crate::compose::build_order(&ctx, "test").unwrap().remove(0);
        (temp, ctx, result)
    }

    #[cfg(unix)]
    #[test]
    fn forced_pull_rejects_content_kind_and_tree_structure_changes() {
        for entry in [
            r#"{name="node",from_config={key="value"},format="json"}"#,
            r#"{name="node",from_file="tree"}"#,
        ] {
            let (_temp, ctx, result) = adoption_fixture(entry);
            let source = ctx.orders_dir.join("test/order.ncl");
            let before = std::fs::read_to_string(&source).unwrap();
            if result.source_is_directory() {
                std::fs::write(&result.target, "regular target").unwrap();
            } else {
                std::os::unix::fs::symlink("dangling", &result.target).unwrap();
            }
            let error = sync_resource(
                &ctx,
                "test",
                0,
                &result,
                SyncMode::ApplyTargetToSourceAll,
                false,
                &crate::sync::TerminalPrompter,
            )
            .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("forced Target -> Source cannot change resource kind")
            );
            assert_eq!(std::fs::read_to_string(source).unwrap(), before);
            assert!(ctx.orders_dir.join("test/tree/file").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn same_kind_symlink_pull_is_allowed_and_formatted() {
        let (_temp, ctx, result) =
            adoption_fixture(r#"{name="node",kind="symlink",link_to="old"}"#);
        std::os::unix::fs::symlink("new#fragment", &result.target).unwrap();
        assert_eq!(
            sync_resource(
                &ctx,
                "test",
                0,
                &result,
                SyncMode::ApplyTargetToSourceAll,
                false,
                &crate::sync::TerminalPrompter,
            )
            .unwrap(),
            SyncAction::ApplyTargetToSource
        );
        let source = std::fs::read_to_string(ctx.orders_dir.join("test/order.ncl")).unwrap();
        assert!(source.contains("new#fragment"));
        assert_eq!(source, crate::nickel::format_source(&source).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn adoption_distinguishes_hash_strings_from_comments_and_formats_result() {
        let (_temp, ctx, result) =
            adoption_fixture(r##"{name="node",from_config="#!/bin/sh",format="plaintext"}"##);
        std::os::unix::fs::symlink("new", &result.target).unwrap();
        let adoption = adopted_declaration(&ctx, "test", 0, &result).unwrap();
        assert_eq!(
            adoption.updated,
            crate::nickel::format_source(&adoption.updated).unwrap()
        );
        assert!(!adoption.updated.contains("#!/bin/sh"));
        assert!(
            declaration_comment_ranges(r##"{x="a#fragment", y="#!/bin/sh"}"##)
                .unwrap()
                .is_empty()
        );
        let comments = declaration_comment_ranges("{x = { # preserve this\n value=1},}").unwrap();
        assert_eq!(comments.len(), 1);
        let path = ctx.orders_dir.join("test/order.ncl");
        std::fs::write(
            path,
            r#"{blend={prefix=["~/"],files=[{name="node",from_config={ # preserve this
value=1},format="json"}]}}"#,
        )
        .unwrap();
        assert!(
            adopted_declaration(&ctx, "test", 0, &result)
                .err()
                .unwrap()
                .to_string()
                .contains("contains comments")
        );
    }

    #[cfg(unix)]
    #[test]
    fn adoption_refuses_to_drop_meaningful_content_settings() {
        for option in [
            r#"immutable=true"#,
            r#"ignore=["key"]"#,
            r#"local="overlay""#,
            r#"exclude=["*.bak"]"#,
        ] {
            let entry = format!(r#"{{name="node",from_file="tree",{option}}}"#);
            let (_temp, ctx, result) = adoption_fixture(&entry);
            std::os::unix::fs::symlink("new", &result.target).unwrap();
            let error = adopted_declaration(&ctx, "test", 0, &result).err().unwrap();
            assert!(
                error.to_string().contains("would discard"),
                "{option}: {error}"
            );
        }
    }

    #[test]
    fn source_edit_is_atomic_and_rejects_stale_or_failed_validation() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("order.ncl");
        std::fs::write(&path, "original").unwrap();
        let failed = atomic_source_edit(&path, "original", "replacement", || {
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
            bail!("Target changed");
        });
        assert!(failed.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        let stale = atomic_source_edit(&path, "original", "replacement", || {
            std::fs::write(&path, "newer editor content")?;
            Ok(())
        });
        assert!(stale.unwrap_err().to_string().contains("Source changed"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "newer editor content"
        );
        atomic_source_edit(&path, "newer editor content", "complete", || Ok(())).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "complete");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn concurrent_adoptions_do_not_commit_two_stale_proposals() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("order.ncl");
        std::fs::write(&path, "original").unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = ["first", "second"]
            .into_iter()
            .map(|value| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    atomic_source_edit(&path, "original", value, || Ok(()))
                })
            })
            .collect();
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(["first", "second"].contains(&std::fs::read_to_string(path).unwrap().as_str()));
    }

    #[cfg(unix)]
    #[test]
    fn frozen_tree_manifest_does_not_deploy_newly_added_conflicting_paths() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("source");
        let home = temp.path().join("home");
        let order = root.join("orders/test");
        std::fs::create_dir_all(order.join("tree")).unwrap();
        std::fs::create_dir_all(home.join("tree")).unwrap();
        std::fs::write(order.join("tree/first"), "validated").unwrap();
        std::fs::write(home.join("tree/other"), "other owner").unwrap();
        std::fs::set_permissions(
            home.join("tree/other"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        std::fs::write(order.join("order.ncl"), r#"{blend={prefix=["~/"],files=[{name="tree",from_file="tree",mode="0600"},{name="tree/other",from_config="other owner",format="plaintext"}]}}"#).unwrap();
        let cli = crate::cli::Cli::parse_from([
            "blend",
            "--blend-dir",
            root.to_str().unwrap(),
            "--home",
            home.to_str().unwrap(),
        ]);
        let ctx = Context::new(&cli).unwrap();
        let ownership = validate_ownership(&ctx).unwrap();
        std::fs::write(order.join("tree/other"), "must not overwrite").unwrap();
        let mut built = crate::compose::build_order(&ctx, "test").unwrap();
        let tree = built
            .iter_mut()
            .find(|result| result.name == "tree")
            .unwrap();
        bind_source_manifest(&ownership, tree).unwrap();
        crate::compose::write_result(tree, false).unwrap();
        crate::compose::reconcile_file_mode(tree, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join("tree/first")).unwrap(),
            "validated"
        );
        assert_eq!(
            std::fs::read_to_string(home.join("tree/other")).unwrap(),
            "other owner"
        );
        assert!(
            !crate::commands::helpers::compute_content_diff_for_result(tree)
                .unwrap()
                .has_changes
        );
        std::fs::write(home.join("tree/first"), "pulled through frozen manifest").unwrap();
        crate::sync::pull_build_result(tree, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(order.join("tree/first")).unwrap(),
            "pulled through frozen manifest"
        );
        assert_eq!(
            std::fs::read_to_string(order.join("tree/other")).unwrap(),
            "must not overwrite"
        );
        assert!(
            !crate::commands::helpers::compute_content_diff_for_result(tree)
                .unwrap()
                .has_changes
        );
        std::fs::create_dir(order.join("different")).unwrap();
        let declaration = std::fs::read_to_string(order.join("order.ncl"))
            .unwrap()
            .replace("from_file=\"tree\"", "from_file=\"different\"");
        std::fs::write(order.join("order.ncl"), declaration).unwrap();
        let mut changed = crate::compose::build_order(&ctx, "test").unwrap();
        let changed_tree = changed
            .iter_mut()
            .find(|result| result.name == "tree")
            .unwrap();
        assert!(
            bind_source_manifest(&ownership, changed_tree)
                .unwrap_err()
                .to_string()
                .contains("Source declaration changed")
        );
        assert_eq!(
            std::fs::metadata(home.join("tree/other"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o644
        );
    }
    #[cfg(unix)]
    #[test]
    fn changed_target_aborts_adoption_without_rewriting_source_or_following_links() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("source");
        let home = temp.path().join("home");
        let order = root.join("orders/test");
        std::fs::create_dir_all(&order).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let path = order.join("order.ncl");
        let original =
            r#"{blend={prefix=["~/"],files=[{name="node",kind="symlink",link_to="chosen"}]}}"#;
        std::fs::write(&path, original).unwrap();
        let cli = crate::cli::Cli::parse_from([
            "blend",
            "--blend-dir",
            root.to_str().unwrap(),
            "--home",
            home.to_str().unwrap(),
        ]);
        let ctx = Context::new(&cli).unwrap();
        let mut resource = crate::compose::build_order(&ctx, "test").unwrap().remove(0);
        let target = home.join("node");
        std::os::unix::fs::symlink("chosen", &target).unwrap();
        validate_adoption_target(&resource).unwrap();
        std::fs::remove_file(&target).unwrap();
        std::os::unix::fs::symlink("changed", &target).unwrap();
        let error = atomic_source_edit(&path, original, "replacement", || {
            validate_adoption_target(&resource)
        })
        .unwrap_err();
        assert!(error.to_string().contains("Target changed"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let backing = home.join("backing");
        std::fs::write(&backing, "chosen").unwrap();
        std::fs::remove_file(&target).unwrap();
        std::os::unix::fs::symlink(&backing, &target).unwrap();
        resource.kind = ResourceKind::Content;
        resource.content = "chosen".to_owned();
        assert!(validate_adoption_target(&resource).is_err());
        assert_eq!(std::fs::read_to_string(&backing).unwrap(), "chosen");
    }
}
