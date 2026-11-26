use std::io::{self, Write};

/// The website and binary consume the same maintained source asset.
pub const CONTENT: &str = include_str!("../skills/blend-orders/SKILL.md");

pub fn print() -> io::Result<()> {
    io::stdout().lock().write_all(CONTENT.as_bytes())
}
