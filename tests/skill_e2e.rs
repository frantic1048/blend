//! Exercise the shipped instructions as well as the context-free print command.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

const SKILL: &str = include_str!("../skills/blend-orders/SKILL.md");

struct Fixture {
    root: TempDir,
    home: PathBuf,
    source: PathBuf,
    cwd: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let base = root.path().canonicalize().unwrap();
        let home = base.join("home");
        let source = base.join("source");
        let cwd = base.join("elsewhere");
        let state = base.join("state");
        for path in [&home, &source, &cwd] {
            std::fs::create_dir(path).unwrap();
        }
        Self {
            root,
            home,
            source,
            cwd,
            state,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_blend"));
        command
            .current_dir(&self.cwd)
            .env("HOME", &self.home)
            .env("XDG_STATE_HOME", &self.state)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command()
            .arg("--blend-dir")
            .arg(&self.source)
            .args(args)
            .output()
            .unwrap()
    }

    fn order(&self, content: &str) -> PathBuf {
        let orders = self.source.join("orders");
        std::fs::create_dir_all(orders.join("editor")).unwrap();
        for (name, bytes) in [
            (
                "order.contract.ncl",
                include_bytes!("../src/nickel/assets/order.contract.ncl").as_slice(),
            ),
            (
                "metadata.ncl",
                include_bytes!("../src/nickel/assets/metadata.ncl").as_slice(),
            ),
        ] {
            std::fs::write(orders.join(name), bytes).unwrap();
        }
        let path = orders.join("editor/order.ncl");
        std::fs::write(&path, content).unwrap();
        path
    }

    fn target(&self) -> PathBuf {
        let path = self.home.join(".config/editor/settings.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        path
    }
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

// Include directories so creating an otherwise empty state directory is a write.
fn tree(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let mut entries: Vec<_> = walkdir::WalkDir::new(root)
        .into_iter()
        .map(Result::unwrap)
        .map(|entry| {
            (
                entry.path().strip_prefix(root).unwrap().to_path_buf(),
                entry
                    .file_type()
                    .is_file()
                    .then(|| std::fs::read(entry.path()).unwrap()),
            )
        })
        .collect();
    entries.sort();
    entries
}

fn first_order(markdown: &str) -> &str {
    markdown
        .split_once("```nickel\n")
        .unwrap()
        .1
        .split_once("```")
        .unwrap()
        .0
}

#[test]
fn print_is_exact_before_init_and_independent_of_config_state_and_flags() {
    let f = Fixture::new();
    for broken in [false, true] {
        if broken {
            let config = f.home.join(".config/blend/config.toml");
            std::fs::create_dir_all(config.parent().unwrap()).unwrap();
            std::fs::write(config, "not valid TOML [").unwrap();
            let state = f.state.join("blend/state.json");
            std::fs::create_dir_all(state.parent().unwrap()).unwrap();
            std::fs::write(state, "not valid JSON").unwrap();
        }
        let before = tree(f.root.path());
        for args in [
            vec!["skill", "print"],
            vec!["--verbose", "--dry-run", "skill", "print"],
            vec!["skill", "print", "--sandbox", "force"],
        ] {
            let output = f.command().args(&args).output().unwrap();
            success(&output);
            assert_eq!(output.stdout, SKILL.as_bytes(), "{args:?}");
            assert!(output.stderr.is_empty(), "{args:?}");
        }
        assert_eq!(tree(f.root.path()), before);
    }

    // Printing does not even need HOME or a valid --blend-dir.
    let output = f
        .command()
        .env_remove("HOME")
        .args(["--blend-dir", "/does/not/exist", "skill", "print"])
        .output()
        .unwrap();
    success(&output);
    assert_eq!(output.stdout, SKILL.as_bytes());
}

#[test]
fn help_discovers_authoring_and_bare_skill_shows_subcommands() {
    let f = Fixture::new();
    let output = f.command().args(["--help"]).output().unwrap();
    success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("skill print"));

    let output = f.command().args(["add", "--help"]).output().unwrap();
    success(&output);
    let help = String::from_utf8_lossy(&output.stdout);
    for text in [
        "Import",
        "from_file",
        "orders/<name>/order.ncl",
        "blend skill print",
    ] {
        assert!(help.contains(text), "missing {text}: {help}");
    }

    // Clap uses its usual missing-subcommand status, and emits help rather than
    // entering runtime setup (this fixture has neither orders nor state).
    let output = f.command().args(["skill"]).output().unwrap();
    let help = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        help.contains("Usage: blend skill ") && help.contains("<COMMAND>"),
        "{help}"
    );
    assert!(help.contains("print"), "{help}");
    assert!(!f.state.exists());
}

#[test]
fn documented_orders_preview_then_preserve_application_fields_when_synced() {
    for order in [
        first_order(SKILL),
        first_order(include_str!("../README.md")),
        first_order(include_str!("../docs/GUIDE.md")),
    ] {
        let f = Fixture::new();
        let order_path = f.order(order);
        let target = f.target();
        let original = "# Application settings\ntheme = 'light'\nrecent = ['one', 'two']\n[window]\nwidth = 1200\n[window.position]\nx = 40\n";
        std::fs::write(&target, original).unwrap();
        let before = tree(f.root.path());

        success(&f.run(&["check", "editor"]));
        let preview = f.run(&["view", "editor"]);
        success(&preview);
        let diff = String::from_utf8_lossy(&preview.stdout);
        assert!(diff.contains("theme") && diff.contains("dark"), "{diff}");
        assert!(
            !diff.contains("width") && !diff.contains("recent"),
            "{diff}"
        );
        assert_eq!(tree(f.root.path()), before);

        // Enforced fields reconcile without a prompt, even with stdin closed.
        success(&f.run(&["sync", "editor"]));
        let mut expected: toml::Value = toml::from_str(original).unwrap();
        expected["theme"] = toml::Value::String("dark".into());
        let deployed: toml::Value =
            toml::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(deployed, expected);
        assert_eq!(std::fs::read_to_string(&order_path).unwrap(), order);

        std::fs::remove_file(&target).unwrap();
        success(&f.run(&["sync", "editor"]));
        let deployed: toml::Value =
            toml::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(
            deployed,
            toml::from_str::<toml::Value>("theme = 'dark'").unwrap()
        );
    }
}

#[test]
fn computed_import_diagnostic_points_to_direct_authoring() {
    let f = Fixture::new();
    let order_path = f.order(
        r#"let { Order, .. } = import "../order.contract.ncl" in
{ blend = { prefix = ["~/.config/editor/"], files = if true then [] else [] } } | Order"#,
    );
    let target = f.target();
    std::fs::write(&target, "theme = 'light'\n").unwrap();
    let before = tree(f.root.path());
    let output = f.run(&["add", "editor", target.to_str().unwrap()]);
    assert!(!output.status.success());
    let message = String::from_utf8_lossy(&output.stderr);
    for text in [
        order_path.to_str().unwrap(),
        "blend check editor",
        "blend view editor",
        "blend skill print",
    ] {
        assert!(message.contains(text), "missing {text}: {message}");
    }
    assert_eq!(tree(f.root.path()), before);
}
