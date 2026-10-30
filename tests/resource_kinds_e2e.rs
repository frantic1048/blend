#![cfg(unix)]
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

struct Fixture {
    home: TempDir,
    repo: TempDir,
}
impl Fixture {
    fn new(files: &str) -> Self {
        let f = Self {
            home: TempDir::new().unwrap(),
            repo: TempDir::new().unwrap(),
        };
        let orders = f.repo.path().join("orders");
        std::fs::create_dir_all(orders.join("test")).unwrap();
        std::fs::write(
            orders.join("order.contract.ncl"),
            include_str!("../src/nickel/assets/order.contract.ncl"),
        )
        .unwrap();
        std::fs::write(
            orders.join("metadata.ncl"),
            include_str!("../src/nickel/assets/metadata.ncl"),
        )
        .unwrap();
        f.order(files);
        f
    }
    fn order(&self, files: &str) {
        std::fs::write(
            self.source(),
            format!("{{ blend = {{ prefix = [\"~/\"], files = [{files}], }}, }}"),
        )
        .unwrap();
    }
    fn source(&self) -> std::path::PathBuf {
        self.repo.path().join("orders/test/order.ncl")
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_blend"))
            .args(args)
            .arg("--home")
            .arg(self.home.path())
            .arg("--blend-dir")
            .arg(self.repo.path())
            .env("XDG_STATE_HOME", self.repo.path().join("state"))
            .output()
            .unwrap()
    }
    fn interactive(&self, input: &str) -> String {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = Command::new(env!("CARGO_BIN_EXE_blend"))
            .args(["sync", "test", "--home"])
            .arg(self.home.path())
            .arg("--blend-dir")
            .arg(self.repo.path())
            .env("XDG_STATE_HOME", self.repo.path().join("state"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}
fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

#[test]
fn literal_links_preserve_destination_text_without_following_referents() {
    let f =
        Fixture::new(r#"{ name = "current", kind = "symlink", link_to = "../missing//./latest" }"#);
    f.ok(&["check"]);
    f.ok(&["sync", "test"]);
    let link = f.home.path().join("current");
    assert_eq!(
        std::fs::read_link(&link).unwrap().as_os_str(),
        "../missing//./latest"
    );
    assert!(f.ok(&["status"]).contains("linked"));
    assert!(f.ok(&["view", "test"]).contains("../missing//./latest"));
    std::fs::remove_file(&link).unwrap();
    symlink("../missing/latest", &link).unwrap();
    assert!(f.ok(&["status"]).contains("mismatch"));
    f.ok(&["sync", "--dry-run", "--force-source-to-target", "test"]);
    assert_eq!(
        std::fs::read_link(&link).unwrap().as_os_str(),
        "../missing/latest"
    );
    f.ok(&["sync", "--force-source-to-target", "test"]);
    assert_eq!(
        std::fs::read_link(&link).unwrap().as_os_str(),
        "../missing//./latest"
    );
}

#[test]
fn directory_resources_preserve_contents_allow_descendants_and_enforce_only_explicit_mode() {
    let f = Fixture::new(
        r#"{ name = "cache", kind = "directory", mode = "0700" }, { name = "cache/managed", from_config = "hello", format = "plaintext" }"#,
    );
    std::fs::create_dir(f.home.path().join("cache")).unwrap();
    std::fs::write(f.home.path().join("cache/unmanaged"), "keep").unwrap();
    f.ok(&["sync", "test"]);
    assert_eq!(mode(&f.home.path().join("cache")), 0o700);
    assert_eq!(
        std::fs::read_to_string(f.home.path().join("cache/unmanaged")).unwrap(),
        "keep"
    );
    assert_eq!(
        std::fs::read_to_string(f.home.path().join("cache/managed")).unwrap(),
        "hello"
    );
    f.order(r#"{ name = "cache", kind = "directory" }"#);
    std::fs::set_permissions(
        f.home.path().join("cache"),
        std::fs::Permissions::from_mode(0o750),
    )
    .unwrap();
    f.ok(&["sync", "--force-source-to-target", "test"]);
    assert_eq!(mode(&f.home.path().join("cache")), 0o750);
    assert!(f.ok(&["status"]).contains("directory"));
    assert!(f.ok(&["view", "test"]).contains("in sync"));
    f.order(r#"{ name = "cache", kind = "directory", mode = "0700" }"#);
    assert!(f.ok(&["status"]).contains("mismatch"));
    assert!(f.ok(&["view", "test"]).contains("0750 -> 0700"));
    f.ok(&["sync", "--force-target-to-source", "test"]);
    assert_eq!(mode(&f.home.path().join("cache")), 0o700);
}

#[test]
fn non_empty_directories_are_never_recursively_removed_even_with_force_or_dry_run() {
    for dry in [false, true] {
        let f = Fixture::new(r#"{ name = "cache", kind = "symlink", link_to = "elsewhere" }"#);
        std::fs::create_dir(f.home.path().join("cache")).unwrap();
        std::fs::write(f.home.path().join("cache/keep"), "unmanaged").unwrap();
        let args = if dry {
            vec!["sync", "--dry-run", "--force-source-to-target", "test"]
        } else {
            vec!["sync", "--force-source-to-target", "test"]
        };
        let out = f.run(&args);
        assert!(!out.status.success());
        assert!(
            format!(
                "{} {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
            .contains("non-empty directory")
        );
        assert_eq!(
            std::fs::read_to_string(f.home.path().join("cache/keep")).unwrap(),
            "unmanaged"
        );
    }
}

#[test]
fn link_adoption_converts_source_reference_to_literal_without_modifying_source_or_referent() {
    let f = Fixture::new(r#"{ from_file = "source", kind = "symlink" }"#);
    let source = f.repo.path().join("orders/test/source");
    std::fs::write(&source, "source bytes").unwrap();
    let backing = f.home.path().join("backing");
    std::fs::write(&backing, "referent bytes").unwrap();
    symlink("./backing", f.home.path().join("source")).unwrap();
    let before = std::fs::read_to_string(f.source()).unwrap();
    f.ok(&["sync", "--dry-run", "--force-target-to-source", "test"]);
    assert_eq!(std::fs::read_to_string(f.source()).unwrap(), before);
    f.ok(&["sync", "--force-target-to-source", "test"]);
    let updated = std::fs::read_to_string(f.source()).unwrap();
    assert!(updated.contains("link_to = \"./backing\""), "{updated}");
    assert!(!updated.contains("from_file"));
    assert!(updated.contains("name = \"source\""));
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "source bytes");
    assert_eq!(std::fs::read_to_string(&backing).unwrap(), "referent bytes");
    f.ok(&["check"]);
    f.ok(&["format", "--check"]);
    assert!(f.ok(&["status"]).contains("linked"));
}

#[test]
fn interactive_kind_conversions_do_not_import_a_links_referent() {
    let f = Fixture::new(r#"{ name = "node", from_config = "original", format = "plaintext" }"#);
    symlink("absent", f.home.path().join("node")).unwrap();
    f.interactive("t\n");
    let source = std::fs::read_to_string(f.source()).unwrap();
    assert!(source.contains("link_to = \"absent\""), "{source}");
    assert!(!source.contains("from_config"));
    std::fs::remove_file(f.home.path().join("node")).unwrap();
    std::fs::write(f.home.path().join("node"), "target file\n").unwrap();
    f.interactive("t\n");
    assert!(
        std::fs::read_to_string(f.source())
            .unwrap()
            .contains("kind = \"content\"")
    );
    f.ok(&["sync", "--force-source-to-target", "test"]);
    assert_eq!(
        std::fs::read_to_string(f.home.path().join("node")).unwrap(),
        "target file\n"
    );
}

#[test]
fn computed_resource_declarations_report_manual_edit_without_claiming_success() {
    let f = Fixture::new("");
    let original = r#"{ blend = { prefix = ["~/"], files = if true then [{name="node", kind="symlink", link_to="old"}] else [] } }"#;
    std::fs::write(f.source(), original).unwrap();
    symlink("new", f.home.path().join("node")).unwrap();
    let out = f.run(&["sync", "--force-target-to-source", "test"]);
    assert!(!out.status.success());
    assert!(
        format!(
            "{} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
        .contains("edit order.ncl manually")
    );
    assert_eq!(std::fs::read_to_string(f.source()).unwrap(), original);
}

#[test]
fn duplicate_targets_fail_before_writing_and_inactive_entries_do_not_collide() {
    let f = Fixture::new(
        r#"{ name = "node", kind = "directory" }, { name = "./node", kind = "symlink", link_to = "absent" }"#,
    );
    let out = f.run(&["sync", "--force-source-to-target", "test"]);
    assert!(!out.status.success());
    assert!(
        format!(
            "{} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
        .contains("Duplicate Target")
    );
    assert!(!f.home.path().join("node").exists());
    f.order(r#"{ name = "node", kind = "directory" }, { name = "node", kind = "symlink", link_to = "absent", when = { os = ["unsupported"] } }"#);
    f.ok(&["sync", "test"]);
    assert!(f.home.path().join("node").is_dir());
}

#[test]
fn kind_specific_invalid_inputs_are_rejected() {
    for files in [
        r#"{ name="x", kind="unknown", from_config="x" }"#,
        r#"{ name="x", kind="directory", from_file="x" }"#,
        r#"{ name="x", kind="directory", immutable=true }"#,
        r#"{ name="x", kind="symlink" }"#,
        r#"{ name="x", kind="symlink", link_to="a", from_file="b" }"#,
        r#"{ name="x", kind="symlink", link_to="" }"#,
        r#"{ name="x", kind="symlink", link_to="a", mode="0700" }"#,
        r#"{ name="x", kind="content", symlink=true, from_file="x" }"#,
        r#"{ name="x", link_to="a", from_config="x" }"#,
        r#"{ name="x", kind="directory", mode="700" }"#,
        r#"{ name="x", symlink=true, link_to="a" }"#,
    ] {
        let f = Fixture::new(files);
        let out = f.run(&["check"]);
        assert!(!out.status.success(), "accepted {files}");
        assert!(!f.home.path().join("x").exists());
    }
}

#[test]
fn adoption_preserves_comments_and_works_with_standard_contract_annotation() {
    let f = Fixture::new("");
    let original = r#"let { Order, .. } = import "../order.contract.ncl" in
{
  blend = {
    prefix = ["~/"],
    files = [{
      # A resource comment.
      name = "node", # Keep this name comment.
      kind = "symlink",
      link_to = "old" # Keep destination documentation.
    }],
  },
} | Order
"#;
    std::fs::write(f.source(), original).unwrap();
    symlink("new%{literal}", f.home.path().join("node")).unwrap();
    f.ok(&["sync", "--force-target-to-source", "test"]);
    let updated = std::fs::read_to_string(f.source()).unwrap();
    assert!(updated.contains("# A resource comment."));
    assert!(updated.contains("# Keep this name comment."));
    assert!(updated.contains("# Keep destination documentation."));
    f.ok(&["check"]);
    f.ok(&["format", "--check"]);
    assert!(f.ok(&["status"]).contains("linked"));
}

#[test]
fn resource_dry_runs_do_not_claim_applied_or_rewrite_source() {
    let f = Fixture::new(r#"{ name="node", kind="symlink", link_to="old" }"#);
    symlink("new", f.home.path().join("node")).unwrap();
    let before = std::fs::read_to_string(f.source()).unwrap();
    for direction in ["--force-source-to-target", "--force-target-to-source"] {
        let out = f.ok(&["sync", "--dry-run", direction, "test"]);
        assert!(!out.contains("Applied"), "{out}");
        assert_eq!(std::fs::read_to_string(f.source()).unwrap(), before);
        assert_eq!(
            std::fs::read_link(f.home.path().join("node")).unwrap(),
            Path::new("new")
        );
    }
    let out = f.run(&["sync", "--no-rewrite", "--force-target-to-source", "test"]);
    assert!(!out.status.success());
    assert_eq!(std::fs::read_to_string(f.source()).unwrap(), before);
}

#[test]
fn multi_target_resource_adoption_requires_an_explicit_shared_declaration_edit() {
    let f =
        Fixture::new(r#"{ name="node", prefix=["~/a", "~/b"], kind="symlink", link_to="old" }"#);
    std::fs::create_dir(f.home.path().join("a")).unwrap();
    std::fs::create_dir(f.home.path().join("b")).unwrap();
    symlink("new", f.home.path().join("a/node")).unwrap();
    symlink("old", f.home.path().join("b/node")).unwrap();
    let before = std::fs::read_to_string(f.source()).unwrap();
    let out = f.run(&["sync", "--force-target-to-source", "test"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("multiple Targets"));
    assert_eq!(std::fs::read_to_string(f.source()).unwrap(), before);
}

#[test]
fn declared_modes_survive_adoption_of_regular_content() {
    let f = Fixture::new(r#"{ name="node", kind="directory", mode="0700" }"#);
    let target = f.home.path().join("node");
    std::fs::write(&target, "content").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
    f.interactive("t\n");
    let source = std::fs::read_to_string(f.source()).unwrap();
    assert!(
        source.contains("mode=\"0700\"") || source.contains("mode = \"0700\""),
        "{source}"
    );
    assert_eq!(mode(&target), 0o700);
    f.ok(&["check"]);
}

#[test]
fn directory_resource_replaces_only_the_exact_symlink_and_preserves_parent_links() {
    let f = Fixture::new(r#"{ name="parent/cache", kind="directory", mode="0700" }"#);
    let backing = TempDir::new().unwrap();
    let external = TempDir::new().unwrap();
    std::fs::write(external.path().join("keep"), "external").unwrap();
    std::fs::set_permissions(external.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    symlink(backing.path(), f.home.path().join("parent")).unwrap();
    symlink(external.path(), backing.path().join("cache")).unwrap();
    f.ok(&["sync", "--force-source-to-target", "test"]);
    assert!(
        f.home
            .path()
            .join("parent")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        backing
            .path()
            .join("cache")
            .symlink_metadata()
            .unwrap()
            .is_dir()
    );
    assert_eq!(mode(&backing.path().join("cache")), 0o700);
    assert_eq!(mode(external.path()), 0o755);
    assert_eq!(
        std::fs::read_to_string(external.path().join("keep")).unwrap(),
        "external"
    );
}

#[test]
fn restrictive_directory_modes_are_applied_after_all_descendants() {
    let f = Fixture::new(
        r#"{name="cache",kind="directory",mode="0000"}, {name="cache/managed",from_config="content",format="plaintext"}"#,
    );
    f.ok(&["sync", "test"]);
    let cache = f.home.path().join("cache");
    assert_eq!(mode(&cache), 0);
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        std::fs::read_to_string(cache.join("managed")).unwrap(),
        "content"
    );
}

#[test]
fn structural_adoption_discards_only_the_exact_old_content_snapshot() {
    let f = Fixture::new(r#"{ name="node",from_config="original",format="plaintext" }"#);
    f.ok(&["sync", "test"]);
    let target = f.home.path().join("node");
    let snapshot = f
        .repo
        .path()
        .join("state/blend/snapshots/test")
        .join(target.strip_prefix("/").unwrap());
    assert!(snapshot.is_file());
    std::fs::remove_file(&target).unwrap();
    symlink("absent", &target).unwrap();
    f.interactive("t\n");
    assert!(!snapshot.exists());
    assert_eq!(std::fs::read_link(&target).unwrap(), Path::new("absent"));
}

#[test]
fn declared_directory_access_is_repaired_before_descendant_reconciliation() {
    let f = Fixture::new(
        r#"{name="cache",kind="directory",mode="0700"}, {name="cache/managed",from_config="content",format="plaintext"}"#,
    );
    let cache = f.home.path().join("cache");
    std::fs::create_dir(&cache).unwrap();
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0)).unwrap();
    let out = f.run(&["sync", "--force-source-to-target", "test"]);
    // Restore access even if the assertion fails, for temp-directory cleanup.
    let actual_mode = mode(&cache);
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(actual_mode, 0o700);
    assert_eq!(
        std::fs::read_to_string(cache.join("managed")).unwrap(),
        "content"
    );
}

#[test]
fn selected_descendants_temporarily_open_and_restore_restrictive_directories_across_orders() {
    let f = Fixture::new(r#"{name="cache",kind="directory",mode="0000"}"#);
    let child_order = f.repo.path().join("orders/child");
    std::fs::create_dir(&child_order).unwrap();
    let child_source = child_order.join("order.ncl");
    std::fs::write(&child_source, r#"{blend={prefix=["~/cache"],files=[{name="managed",from_config="first",format="plaintext",mode="0000"}]}}"#).unwrap();
    f.ok(&["sync"]);
    let cache = f.home.path().join("cache");
    assert_eq!(mode(&cache), 0);
    std::fs::write(&child_source, r#"{blend={prefix=["~/cache"],files=[{name="managed",from_config="second",format="plaintext",mode="0000"}]}}"#).unwrap();
    let out = f.run(&["sync", "--force-source-to-target", "child"]);
    let final_mode = mode(&cache);
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(final_mode, 0);
    assert_eq!(mode(&cache.join("managed")), 0);
    std::fs::set_permissions(
        cache.join("managed"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(cache.join("managed")).unwrap(),
        "second"
    );
}

#[test]
fn managed_non_directory_ancestors_reject_descendants_before_any_mutation() {
    for parent in [
        r#"{name="tree",kind="symlink",link_to="outside"}"#,
        r#"{name="tree",from_config="content",format="plaintext"}"#,
    ] {
        let f = Fixture::new(parent);
        let outside = f.home.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        symlink(&outside, f.home.path().join("tree")).unwrap();
        let child_order = f.repo.path().join("orders/child");
        std::fs::create_dir(&child_order).unwrap();
        std::fs::write(child_order.join("order.ncl"), r#"{blend={prefix=["~/"],files=[{name="tree/file",from_config="bad",format="plaintext"}]}}"#).unwrap();
        let out = f.run(&["sync", "--force-source-to-target", "child"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("Incompatible Target ownership"));
        assert!(!outside.join("file").exists());
        assert!(!f.run(&["check"]).status.success());
    }
}

#[test]
fn content_tree_interior_nodes_participate_in_collision_checks() {
    let f = Fixture::new(
        r#"{name="tree",from_file="source"}, {name="tree/sub",kind="symlink",link_to="outside"}"#,
    );
    std::fs::create_dir_all(f.repo.path().join("orders/test/source/sub")).unwrap();
    std::fs::write(
        f.repo.path().join("orders/test/source/sub/managed"),
        "content",
    )
    .unwrap();
    let out = f.run(&["sync", "test"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Duplicate Target"));
    assert!(!f.home.path().join("tree").exists());
}

#[test]
fn mode_less_directory_resources_preserve_restrictive_permissions_across_child_updates() {
    let f = Fixture::new(
        r#"{name="cache",kind="directory"}, {name="cache/managed",from_config="first",format="plaintext"}"#,
    );
    f.ok(&["sync", "test"]);
    let cache = f.home.path().join("cache");
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0)).unwrap();
    f.order(r#"{name="cache",kind="directory"}, {name="cache/managed",from_config="second",format="plaintext"}"#);
    let out = f.run(&["sync", "--force-source-to-target", "test"]);
    let final_mode = mode(&cache);
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(final_mode, 0);
    assert_eq!(
        std::fs::read_to_string(cache.join("managed")).unwrap(),
        "second"
    );
}

#[test]
fn directory_summary_counts_each_changed_resource_once() {
    let f = Fixture::new(r#"{name="cache",kind="directory",mode="0700"}"#);
    assert!(
        f.ok(&["sync", "test"])
            .contains("1 Source -> Target, 0 Target -> Source, 0 skipped")
    );
    assert!(
        f.ok(&["sync", "test"])
            .contains("0 Source -> Target, 0 Target -> Source, 0 skipped")
    );
    std::fs::set_permissions(
        f.home.path().join("cache"),
        std::fs::Permissions::from_mode(0o750),
    )
    .unwrap();
    assert!(
        f.ok(&["sync", "test"])
            .contains("1 Source -> Target, 0 Target -> Source, 0 skipped")
    );
}

#[test]
fn kind_adoption_preserves_resolution_authority() {
    let f =
        Fixture::new(r#"{name="node",from_config='Enforce "authoritative",format="plaintext"}"#);
    symlink("missing", f.home.path().join("node")).unwrap();
    let before = std::fs::read_to_string(f.source()).unwrap();
    f.ok(&["sync", "--force-target-to-source", "test"]);
    assert_eq!(std::fs::read_to_string(f.source()).unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(f.home.path().join("node")).unwrap(),
        "authoritative"
    );
}

#[test]
fn wrong_kind_managed_ancestors_block_child_only_sync_without_accessing_referents() {
    let f = Fixture::new(r#"{name="cache",kind="directory"}"#);
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("managed"), "outside").unwrap();
    symlink(outside.path(), f.home.path().join("cache")).unwrap();
    let child_order = f.repo.path().join("orders/child");
    std::fs::create_dir(&child_order).unwrap();
    std::fs::write(child_order.join("order.ncl"), r#"{blend={prefix=["~/cache"],files=[{name="managed",from_config="source",format="plaintext"}]}}"#).unwrap();
    let before = std::fs::read_to_string(child_order.join("order.ncl")).unwrap();
    for direction in ["--force-source-to-target", "--force-target-to-source"] {
        let out = f.run(&["sync", direction, "child"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("Managed ancestor"));
        assert_eq!(
            std::fs::read_to_string(outside.path().join("managed")).unwrap(),
            "outside"
        );
        assert_eq!(
            std::fs::read_to_string(child_order.join("order.ncl")).unwrap(),
            before
        );
    }
    f.ok(&["sync", "--force-source-to-target", "test"]);
    assert!(
        f.home
            .path()
            .join("cache")
            .symlink_metadata()
            .unwrap()
            .is_dir()
    );
    f.ok(&["sync", "--force-source-to-target", "child"]);
    assert_eq!(
        std::fs::read_to_string(f.home.path().join("cache/managed")).unwrap(),
        "source"
    );
    assert_eq!(
        std::fs::read_to_string(outside.path().join("managed")).unwrap(),
        "outside"
    );
}

#[test]
fn failed_literal_link_creation_preserves_existing_target() {
    let f = Fixture::new(r#"{name="node",kind="symlink",link_to="placeholder"}"#);
    let target = f.home.path().join("node");
    std::fs::write(&target, "original").unwrap();
    f.order(&format!(
        "{{name=\"node\",kind=\"symlink\",link_to=\"{}\"}}",
        "x".repeat(32768)
    ));
    let out = f.run(&["sync", "--force-source-to-target", "test"]);
    assert!(!out.status.success());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
    assert!(!std::fs::read_dir(f.home.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".blend-")
    }));
}

#[test]
fn ownership_respects_filesystem_case_behavior_and_rejects_unowned_parent_aliases() {
    let f = Fixture::new(
        r#"{name="Config",from_config="first",format="plaintext"},{name="config",from_config="second",format="plaintext"}"#,
    );
    // Determine fixture filesystem behavior independently from Blend.
    std::fs::write(f.home.path().join("CaseProbe"), "probe").unwrap();
    let case_sensitive = !f.home.path().join("caseprobe").exists();
    std::fs::remove_file(f.home.path().join("CaseProbe")).unwrap();
    let out = f.run(&["sync", "--force-source-to-target", "test"]);
    if case_sensitive {
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(f.home.path().join("Config")).unwrap(),
            "first"
        );
        assert_eq!(
            std::fs::read_to_string(f.home.path().join("config")).unwrap(),
            "second"
        );
    } else {
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("case-ambiguous"));
        assert!(!f.home.path().join("config").exists());
    }
    std::fs::create_dir(f.home.path().join("real")).unwrap();
    symlink("real", f.home.path().join("alias")).unwrap();
    f.order(r#"{name="real/node",from_config="first",format="plaintext"},{name="alias/node",from_config="second",format="plaintext"}"#);
    let out = f.run(&["sync", "--force-source-to-target", "test"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Aliased"));
    assert!(!f.home.path().join("real/node").exists());
}

#[test]
fn missing_directory_ancestors_require_parent_reconciliation_and_mode() {
    let f = Fixture::new(r#"{name="cache",kind="directory",mode="0700"}"#);
    let child = f.repo.path().join("orders/child");
    std::fs::create_dir(&child).unwrap();
    std::fs::write(child.join("order.ncl"), r#"{blend={prefix=["~/cache"],files=[{name="managed",from_config="child",format="plaintext"}]}}"#).unwrap();
    let out = f.run(&["sync", "--force-source-to-target", "child"]);
    assert!(!out.status.success());
    assert!(!f.home.path().join("cache").exists());
    f.ok(&[
        "sync",
        "--dry-run",
        "--force-source-to-target",
        "child",
        "test",
    ]);
    assert!(!f.home.path().join("cache").exists());
    f.ok(&["sync", "--force-source-to-target", "child", "test"]);
    assert_eq!(mode(&f.home.path().join("cache")), 0o700);
    assert_eq!(
        std::fs::read_to_string(f.home.path().join("cache/managed")).unwrap(),
        "child"
    );
}

#[test]
fn absence_discards_old_content_snapshots_even_when_target_is_already_absent() {
    let f = Fixture::new(r#"{name="node",from_config="content",format="plaintext"}"#);
    f.ok(&["sync", "test"]);
    let target = f.home.path().join("node");
    let snapshot = f
        .repo
        .path()
        .join("state/blend/snapshots/test")
        .join(target.strip_prefix("/").unwrap());
    assert!(snapshot.is_file());
    f.order(r#"{name="node",from_config='Absent,format="plaintext"}"#);
    f.ok(&["sync", "test"]);
    assert!(!snapshot.exists());
    assert!(!target.exists());
    std::fs::write(&snapshot, "obsolete").unwrap();
    f.ok(&["sync", "test"]);
    assert!(!snapshot.exists());
}

#[test]
fn structural_sync_fails_until_obsolete_snapshot_can_be_invalidated() {
    let f = Fixture::new(r#"{name="node",from_config="old",format="plaintext"}"#);
    f.ok(&["sync", "test"]);
    let target = f.home.path().join("node");
    let snapshot = f
        .repo
        .path()
        .join("state/blend/snapshots/test")
        .join(target.strip_prefix("/").unwrap());
    let parent = snapshot.parent().unwrap();
    std::fs::remove_file(&target).unwrap();
    f.order(r#"{name="node",kind="symlink",link_to="missing"}"#);
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o500)).unwrap();
    let out = f.run(&["sync", "test"]);
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        !out.status.success(),
        "must fail when obsolete history cannot be removed"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalidate content snapshot"));
    assert!(snapshot.is_file());
    f.ok(&["sync", "test"]);
    assert!(!snapshot.exists());
}

#[test]
fn missing_parent_does_not_block_unmanaged_or_satisfied_absence() {
    let f = Fixture::new(r#"{name="cache",kind="directory",mode="0700"}"#);
    let child = f.repo.path().join("orders/child");
    std::fs::create_dir(&child).unwrap();
    for disposition in ["Unmanaged", "Absent"] {
        std::fs::write(child.join("order.ncl"), format!("{{blend={{prefix=[\"~/cache\"],files=[{{name=\"node\",from_config='{disposition},format=\"plaintext\"}}]}}}}" )).unwrap();
        f.ok(&["sync", "--force-source-to-target", "child"]);
        assert!(!f.home.path().join("cache").exists());
    }
}

#[test]
fn relative_target_prefix_with_missing_parent_remains_valid() {
    let f = Fixture::new(
        r#"{name="config",prefix=["new-parent"],from_config="content",format="plaintext"}"#,
    );
    for args in [
        vec!["check"],
        vec!["sync", "--dry-run", "--force-source-to-target", "test"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_blend"))
            .args(args)
            .arg("--home")
            .arg(f.home.path())
            .arg("--blend-dir")
            .arg(f.repo.path())
            .current_dir(f.home.path())
            .env("XDG_STATE_HOME", f.repo.path().join("state"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert!(!f.home.path().join("new-parent").exists());
}

#[test]
fn interruption_at_prompt_restores_restrictive_directory_permissions() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    for (declared, signal, exit_code) in [
        ("0500", libc::SIGINT, 130),
        ("0555", libc::SIGINT, 130),
        ("0555", libc::SIGTERM, 143),
    ] {
        let f = Fixture::new(&format!(
            r#"{{name="cache",kind="directory",mode="{declared}"}},{{name="cache/file",from_config="source",format="plaintext"}}"#
        ));
        let cache = f.home.path().join("cache");
        std::fs::create_dir(&cache).unwrap();
        std::fs::write(cache.join("file"), "target").unwrap();
        let expected = u32::from_str_radix(declared, 8).unwrap();
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(expected)).unwrap();
        let log_path = f.repo.path().join("prompt.log");
        let mut child = Command::new(env!("CARGO_BIN_EXE_blend"))
            .args(["sync", "test", "--home"])
            .arg(f.home.path())
            .arg("--blend-dir")
            .arg(f.repo.path())
            .env("XDG_STATE_HOME", f.repo.path().join("state"))
            .stdin(Stdio::piped())
            .stdout(std::fs::File::create(&log_path).unwrap())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let prompted = loop {
            if std::fs::read_to_string(&log_path)
                .unwrap()
                .contains("[q]uit:")
            {
                break true;
            }
            if child.try_wait().unwrap().is_some() || Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        // Never leave a hung child if the prompt was not reached.
        unsafe {
            libc::kill(
                child.id() as i32,
                if prompted { signal } else { libc::SIGKILL },
            );
        }
        let stdin = child.stdin.take().unwrap();
        let out = child.wait_with_output().unwrap();
        drop(stdin);
        let restored = mode(&cache);
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(prompted, "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(out.status.code(), Some(exit_code));
        assert_eq!(restored, expected);
        assert_eq!(
            std::fs::read_to_string(cache.join("file")).unwrap(),
            "target"
        );
    }
}

#[test]
fn unrelated_prompts_preserve_entry_order() {
    let f = Fixture::new(
        r#"{name="nested/first",from_config="source",format="plaintext"},{name="second",from_config="source",format="plaintext"}"#,
    );
    std::fs::create_dir(f.home.path().join("nested")).unwrap();
    std::fs::write(f.home.path().join("nested/first"), "target").unwrap();
    std::fs::write(f.home.path().join("second"), "target").unwrap();
    let out = f.interactive("k\nk\n");
    assert!(
        out.find("test:nested/first").unwrap() < out.find("test:second").unwrap(),
        "{out}"
    );
}

#[test]
fn interruption_during_ownership_inventory_stops_before_remaining_orders() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let f = Fixture::new(r#"{name="node",from_config="source",format="plaintext"}"#);
    for index in 0..64 {
        let directory = f.repo.path().join(format!("orders/a{index:03}"));
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("order.ncl"), "{blend={files=[]}}").unwrap();
    }
    let log = f.repo.path().join("startup.log");
    let mut child = Command::new(env!("CARGO_BIN_EXE_blend"))
        .args([
            "sync",
            "--verbose",
            "--force-source-to-target",
            "test",
            "--home",
        ])
        .arg(f.home.path())
        .arg("--blend-dir")
        .arg(f.repo.path())
        .env("XDG_STATE_HOME", f.repo.path().join("state"))
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let started = loop {
        if std::fs::read_to_string(&log)
            .unwrap()
            .contains("Validating ownership for a000")
        {
            break true;
        }
        if child.try_wait().unwrap().is_some() || Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    unsafe {
        libc::kill(
            child.id() as i32,
            if started { libc::SIGINT } else { libc::SIGKILL },
        );
    }
    let out = child.wait_with_output().unwrap();
    let output = std::fs::read_to_string(log).unwrap();
    assert!(started, "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        out.status.code(),
        Some(130),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        output.matches("Validating ownership for").count() < 65,
        "{output}"
    );
    assert!(!f.home.path().join("node").exists());
}
