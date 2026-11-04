//! Relative add operands must behave like their CWD-joined absolute spelling.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

struct Fixture {
    _root: TempDir,
    home: PathBuf,
    source: PathBuf,
    cwd: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        // current_dir reports the physical directory, including on macOS where
        // the temporary-directory prefix may itself be a symlink.
        let base = root.path().canonicalize().unwrap();
        let home = base.join("home");
        let source = home.join("dotfiles");
        let cwd = home.join(".config/editor");
        let state = base.join("state");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(source.join("orders")).unwrap();
        for file in ["order.contract.ncl", "metadata.ncl"] {
            std::fs::copy(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/orders")
                    .join(file),
                source.join("orders").join(file),
            )
            .unwrap();
        }
        std::fs::write(cwd.join("settings.toml"), "theme = 'dark'\n").unwrap();
        std::fs::create_dir(cwd.join("snippets")).unwrap();
        std::fs::write(cwd.join("snippets/example"), "snippet\n").unwrap();
        Self {
            _root: root,
            home,
            source,
            cwd,
            state,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_blend"));
        command
            .arg("--home")
            .arg(&self.home)
            .env("XDG_STATE_HOME", &self.state)
            .current_dir(&self.cwd);
        command
    }

    fn add(&self, source: &Path, operand: &Path, flags: &[&str]) -> Output {
        self.command()
            .arg("--blend-dir")
            .arg(source)
            .args(["add", "editor"])
            .arg(operand)
            .args(flags)
            .output()
            .unwrap()
    }

    fn order(&self) -> PathBuf {
        self.source.join("orders/editor")
    }

    fn remember_source(&self) -> PathBuf {
        let path = self.state.join("blend/state.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({ "blend_dir": self.source })).unwrap(),
        )
        .unwrap();
        path
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn tree(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = walkdir::WalkDir::new(path)
        .into_iter()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            (
                entry.path().strip_prefix(path).unwrap().to_owned(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

#[test]
fn relative_files_and_directories_match_joined_absolute_operands() {
    for operand in ["settings.toml", "./settings.toml", "snippets", "."] {
        let fixture = Fixture::new();
        let absolute_source = fixture.home.join("absolute-source");
        std::fs::create_dir_all(absolute_source.join("orders")).unwrap();
        for file in ["order.contract.ncl", "metadata.ncl"] {
            std::fs::copy(
                fixture.source.join("orders").join(file),
                absolute_source.join("orders").join(file),
            )
            .unwrap();
        }
        let targets_before = tree(&fixture.cwd);
        assert_success(&fixture.add(&fixture.source, Path::new(operand), &[]));
        assert_success(&fixture.add(&absolute_source, &fixture.cwd.join(operand), &[]));
        assert_eq!(
            tree(&fixture.order()),
            tree(&absolute_source.join("orders/editor")),
            "operand: {operand}"
        );
        assert_eq!(tree(&fixture.cwd), targets_before);
    }
}

#[test]
fn relative_operands_retain_prefix_duplicate_and_overlap_checks() {
    let fixture = Fixture::new();
    for (operand, flags, error) in [
        (
            "../editor/settings.toml",
            vec![],
            "pass an absolute or ~-prefixed path without '..'",
        ),
        (
            ".",
            vec!["--prefix", "~/.config/editor"],
            "below the selected prefix",
        ),
        (
            "settings.toml",
            vec!["--prefix", "~/elsewhere"],
            "not under prefix",
        ),
        ("settings.toml", vec!["--prefix", "."], "not under prefix"),
    ] {
        let output = fixture.add(&fixture.source, Path::new(operand), &flags);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(error));
        assert!(!stderr.contains("symlink spelling"));
        assert!(!stderr.contains("using the physical invocation directory"));
        assert!(stderr.matches("without '..'").count() <= 1);
        assert!(!fixture.order().exists());
    }

    for operand in [
        fixture.cwd.join("../editor/settings.toml"),
        PathBuf::from("~/.config/editor/../editor/settings.toml"),
    ] {
        let output = fixture.add(&fixture.source, &operand, &[]);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(stderr.matches("without '..'").count(), 1);
        assert!(!stderr.contains("using the physical invocation directory"));
        assert!(!fixture.order().exists());
    }

    assert_success(&fixture.add(
        &fixture.source,
        Path::new("settings.toml"),
        &["--prefix", "~/.config/editor"],
    ));
    let before = tree(&fixture.source);
    let duplicate = fixture.add(
        &fixture.source,
        Path::new("./settings.toml"),
        &["--prefix", "~/.config/editor"],
    );
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("Entry already exists"));
    // Existing global prefixes discard individual validation errors;
    // the relative-operand hint must still explain the uncollapsed parent path.
    let parent = fixture.add(&fixture.source, Path::new("../editor/settings.toml"), &[]);
    assert!(!parent.status.success());
    assert!(
        String::from_utf8_lossy(&parent.stderr)
            .contains("pass an absolute or ~-prefixed path without '..'")
    );
    assert_eq!(
        String::from_utf8_lossy(&parent.stderr)
            .matches("without '..'")
            .count(),
        1
    );
    let overlap = fixture
        .command()
        .arg("--blend-dir")
        .arg(&fixture.source)
        .args(["add", "other", "settings.toml"])
        .output()
        .unwrap();
    assert!(!overlap.status.success());
    assert!(String::from_utf8_lossy(&overlap.stderr).contains("overlaps existing entries"));
    assert_eq!(tree(&fixture.source), before);
}

#[test]
fn relative_operands_use_invocation_directory_with_source_discovery() {
    for discovery in ["remembered", "ancestor"] {
        let mut fixture = Fixture::new();
        fixture.remember_source();
        if discovery == "ancestor" {
            fixture.cwd = fixture.source.join("work");
            std::fs::create_dir(&fixture.cwd).unwrap();
            std::fs::write(fixture.cwd.join("settings.toml"), "theme = 'dark'\n").unwrap();
        }
        let output = fixture
            .command()
            .args(["add", "editor", "settings.toml"])
            .output()
            .unwrap();
        assert_success(&output);
        let relative = fixture.cwd.strip_prefix(&fixture.home).unwrap();
        assert_eq!(
            std::fs::read(fixture.order().join(relative).join("settings.toml")).unwrap(),
            std::fs::read(fixture.cwd.join("settings.toml")).unwrap(),
        );
        assert!(!fixture.cwd.join("orders").exists());
    }
}

#[test]
fn relative_dry_run_leaves_source_target_and_state_unchanged() {
    let fixture = Fixture::new();
    let state_file = fixture.remember_source();
    let state_before = std::fs::read(&state_file).unwrap();
    let source_before = tree(&fixture.source);
    let target_before = tree(&fixture.cwd);
    let output = fixture
        .command()
        .args(["--dry-run", "add", "editor", "settings.toml"])
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&fixture.cwd.join("settings.toml").display().to_string()));
    assert!(stdout.contains("from_file"));
    assert!(!fixture.order().exists());
    assert_eq!(tree(&fixture.source), source_before);
    assert_eq!(tree(&fixture.cwd), target_before);
    assert_eq!(std::fs::read(state_file).unwrap(), state_before);
    assert!(!fixture.state.join("blend/snapshots").exists());
}

#[cfg(unix)]
#[test]
fn relative_symlink_operands_keep_both_deployment_policies() {
    use std::os::unix::fs::symlink;

    for policy in ["follow", "preserve"] {
        let fixture = Fixture::new();
        let link = fixture.cwd.join("linked.toml");
        symlink("settings.toml", &link).unwrap();
        let rejected = fixture.add(&fixture.source, Path::new("linked.toml"), &[]);
        assert!(!rejected.status.success());
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("--symlink follow"));
        assert!(!fixture.order().exists());
        assert_success(&fixture.add(
            &fixture.source,
            Path::new("linked.toml"),
            &["--symlink", policy],
        ));
        let imported = fixture.order().join(".config/editor/linked.toml");
        assert_eq!(std::fs::read(&imported).unwrap(), b"theme = 'dark'\n");
        let order = std::fs::read_to_string(fixture.order().join("order.ncl")).unwrap();
        assert_eq!(order.contains("kind = \"symlink\""), policy == "preserve");
        std::fs::write(&imported, "theme = 'light'\n").unwrap();
        let sync = fixture
            .command()
            .arg("--blend-dir")
            .arg(&fixture.source)
            .args(["sync", "--force-source-to-target", "editor"])
            .output()
            .unwrap();
        assert_success(&sync);
        assert_eq!(std::fs::read(&link).unwrap(), b"theme = 'light'\n");
        assert_eq!(
            std::fs::symlink_metadata(&link).unwrap().is_symlink(),
            policy == "preserve"
        );
        assert_eq!(
            std::fs::read(fixture.cwd.join("settings.toml")).unwrap(),
            b"theme = 'dark'\n"
        );
    }
}

#[cfg(unix)]
#[test]
fn relative_symlink_traversal_matches_joined_absolute_spelling() {
    use std::os::unix::fs::symlink;

    for operand in [
        "linked/example",
        "linked/",
        "linked/.",
        "linked/../settings.toml",
    ] {
        let fixture = Fixture::new();
        symlink("snippets", fixture.cwd.join("linked")).unwrap();
        let relative = fixture.add(&fixture.source, Path::new(operand), &["--dry-run"]);
        let absolute = fixture.add(&fixture.source, &fixture.cwd.join(operand), &["--dry-run"]);
        assert_eq!(relative.status.code(), absolute.status.code(), "{operand}");
        assert_eq!(relative.stdout, absolute.stdout, "{operand}");
        assert_eq!(relative.stderr, absolute.stderr, "{operand}");
        assert!(!fixture.order().exists());
    }

    let fixture = Fixture::new();
    let alias = fixture.home.join("editor-alias");
    symlink(&fixture.cwd, &alias).unwrap();
    let output = fixture
        .command()
        .current_dir(&alias)
        .env("PWD", &alias)
        .arg("--blend-dir")
        .arg(&fixture.source)
        .args(["add", "editor", "settings.toml"])
        .output()
        .unwrap();
    assert_success(&output);
    assert!(
        fixture
            .order()
            .join(".config/editor/settings.toml")
            .exists()
    );
    assert!(!fixture.order().join("editor-alias").exists());
}

#[cfg(unix)]
#[test]
fn symlinked_home_prefix_failure_explains_physical_cwd_and_tilde_workaround() {
    use std::os::unix::fs::symlink;

    let mut fixture = Fixture::new();
    let alias = fixture.home.with_file_name("home-alias");
    symlink(&fixture.home, &alias).unwrap();
    fixture.home = alias;
    let targets_before = tree(&fixture.cwd);
    let source_before = tree(&fixture.source);

    let rejected = fixture.add(&fixture.source, Path::new("settings.toml"), &[]);
    assert!(!rejected.status.success());
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(stderr.contains("not under prefix ~"));
    assert!(stderr.contains(&fixture.cwd.join("settings.toml").display().to_string()));
    assert!(stderr.contains("using the physical invocation directory"));
    assert!(stderr.contains("pass an absolute or ~-prefixed path matching that prefix"));
    assert!(!fixture.order().exists());
    assert_eq!(tree(&fixture.source), source_before);
    assert!(!fixture.state.exists());

    let parent = fixture.add(&fixture.source, Path::new("../editor/settings.toml"), &[]);
    assert!(!parent.status.success());
    let stderr = String::from_utf8_lossy(&parent.stderr);
    assert!(stderr.contains("using the physical invocation directory"));
    assert_eq!(stderr.matches("without '..'").count(), 1);
    assert!(!stderr.contains("symlink spelling"));

    let alias_prefix = fixture.home.join(".config/editor");
    let prefix = alias_prefix.to_str().unwrap();
    let explicit = fixture.add(
        &fixture.source,
        Path::new("settings.toml"),
        &["--prefix", prefix],
    );
    assert!(!explicit.status.success());
    assert!(String::from_utf8_lossy(&explicit.stderr).contains("different symlink spelling"));
    // Even a symlinked prefix must not suggest a spelling workaround when the
    // Target names the prefix itself rather than a node below it.
    let at_prefix = fixture.add(&fixture.source, Path::new("."), &["--prefix", prefix]);
    assert!(!at_prefix.status.success());
    assert!(!String::from_utf8_lossy(&at_prefix.stderr).contains("symlink spelling"));
    assert_eq!(tree(&fixture.source), source_before);
    assert!(!fixture.state.exists());

    assert_success(&fixture.add(
        &fixture.source,
        Path::new("~/.config/editor/settings.toml"),
        &[],
    ));
    assert_eq!(
        std::fs::read(fixture.order().join(".config/editor/settings.toml")).unwrap(),
        b"theme = 'dark'\n"
    );
    assert_eq!(tree(&fixture.cwd), targets_before);
    let source_after = tree(&fixture.source);
    let existing = fixture.add(&fixture.source, Path::new("settings.toml"), &[]);
    assert!(!existing.status.success());
    assert!(String::from_utf8_lossy(&existing.stderr).contains("different symlink spelling"));
    assert_eq!(tree(&fixture.source), source_after);
}
