use std::process::{Command, Output};
use tempfile::TempDir;

struct Fixture {
    repo: TempDir,
    home: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            repo: TempDir::new().unwrap(),
            home: TempDir::new().unwrap(),
        };
        let orders = fixture.repo.path().join("orders");
        std::fs::create_dir_all(&orders).unwrap();
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
        fixture.order("good", r#"{blend={prefix=["~/"],files=[{name="good",from_config="good",format="plaintext"}]}}"#);
        fixture.order("bad", "unbound_variable");
        fixture
    }

    fn order(&self, name: &str, source: &str) {
        let directory = self.repo.path().join("orders").join(name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("order.ncl"), source).unwrap();
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
}

#[test]
fn invalid_unselected_order_does_not_block_check_or_sync() {
    let fixture = Fixture::new();
    for args in [
        vec!["check", "good"],
        vec!["sync", "--force-source-to-target", "good"],
    ] {
        let output = fixture.run(&args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(
        std::fs::read_to_string(fixture.home.path().join("good")).unwrap(),
        "good"
    );
}

#[test]
fn checking_all_orders_still_reports_each_selected_order() {
    let fixture = Fixture::new();
    let output = fixture.run(&["check", "--verbose"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("good"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("bad:"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Nickel check failed"));
}

#[test]
fn healthy_unselected_order_still_participates_in_collision_validation() {
    let fixture = Fixture::new();
    fixture.order("other", r#"{blend={prefix=["~/"],files=[{name="good",from_config="collision",format="plaintext"}]}}"#);
    for args in [
        vec!["check", "good"],
        vec!["sync", "--force-source-to-target", "good"],
    ] {
        let output = fixture.run(&args);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Duplicate Target"));
    }
    assert!(!fixture.home.path().join("good").exists());
}
