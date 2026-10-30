use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::Deserialize;

/// Output format for rendered configs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    #[default]
    Toml,
    Json,
    Jsonc,
    Yaml,
    SpacePairLines,
    SpaceRecordLines,
    EqualsRecordLines,
    Plaintext,
}

impl Format {
    /// Infer format from file extension
    pub fn from_path(path: &str) -> Self {
        if path.ends_with(".toml") {
            Format::Toml
        } else if path.ends_with(".jsonc") {
            Format::Jsonc
        } else if path.ends_with(".json") {
            Format::Json
        } else if path.ends_with(".yaml") || path.ends_with(".yml") {
            Format::Yaml
        } else {
            Format::Plaintext
        }
    }
}

/// Condition for when to apply this order
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct WhenCondition {
    /// List of allowed operating systems
    #[serde(default)]
    pub os: Vec<String>,
    /// List of allowed architectures
    #[serde(default)]
    pub arch: Vec<String>,
    /// List of allowed hostnames
    #[serde(default)]
    pub hostname: Vec<String>,
}

impl WhenCondition {
    /// Check if condition matches the given metadata
    pub fn matches(&self, os: &str, arch: &str, hostname: &str) -> bool {
        let os_ok = self.os.is_empty() || self.os.iter().any(|o| o == os);
        let arch_ok = self.arch.is_empty() || self.arch.iter().any(|a| a == arch);
        let hostname_ok = self.hostname.is_empty() || self.hostname.iter().any(|h| h == hostname);
        os_ok && arch_ok && hostname_ok
    }

    /// Check if condition matches a platform (os + arch), ignoring hostname
    pub fn matches_platform(&self, os: &str, arch: &str) -> bool {
        let os_ok = self.os.is_empty() || self.os.iter().any(|o| o == os);
        let arch_ok = self.arch.is_empty() || self.arch.iter().any(|a| a == arch);
        os_ok && arch_ok
    }
}

/// The Target resource, independent of its Source input.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    #[default]
    Content,
    Symlink,
    Directory,
}

/// Individual file/folder entry in an order
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct FileEntry {
    /// Omitted for the default content kind; explicit kinds never inferred on import.
    pub kind: Option<ResourceKind>,
    /// Literal link destination. Relative destinations are relative to the Target parent.
    pub link_to: Option<String>,
    /// Destination filename, combined with prefix for target path
    #[serde(default)]
    pub name: String,
    /// External file/folder to copy (plaintext mode)
    pub from_file: Option<String>,
    /// Inline structured config data
    pub from_config: Option<serde_json::Value>,
    /// Destination prefixes (overrides global if set)
    #[serde(default)]
    pub prefix: Vec<String>,
    /// Override format (inferred from name if omitted)
    pub format: Option<Format>,
    /// Keys to exclude from diff
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Per-file condition
    pub when: Option<WhenCondition>,
    /// Create symlink instead of copying (from_file only)
    #[serde(default)]
    pub symlink: bool,
    /// Glob patterns to exclude when shipping a from_file directory
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Path to a local overlay directory (gitignored) that overrides/extends
    /// the main from_file directory. Only valid for directory from_file entries.
    pub local: Option<String>,
    /// Set OS immutable flag on the deployed file after writing
    #[serde(default)]
    pub immutable: bool,
    /// Unix permission mode enforced on managed regular target files.
    /// Written as a four-digit octal string such as "0600".
    pub mode: Option<String>,
}

impl FileEntry {
    /// Validate kind-specific inputs and infer a missing name from from_file.
    /// The deprecated boolean is an alias only when no conflicting kind is set.
    pub fn resolve_defaults(&mut self) -> Result<()> {
        if self.symlink && self.kind.is_some_and(|kind| kind != ResourceKind::Symlink) {
            bail!("'symlink = true' conflicts with explicit 'kind'");
        }
        if self.symlink && self.from_file.is_none() {
            bail!(
                "deprecated 'symlink = true' requires 'from_file'; use kind = \"symlink\" for literal destinations"
            );
        }
        match self.effective_kind() {
            ResourceKind::Content => {
                if self.from_file.is_some() == self.from_config.is_some() {
                    bail!("content entry requires exactly one of 'from_file' and 'from_config'");
                }
                if self.link_to.is_some() {
                    bail!("'link_to' requires kind = \"symlink\"");
                }
            }
            ResourceKind::Symlink => {
                if self.from_config.is_some() {
                    bail!("'symlink' cannot be used with 'from_config'");
                }
                if self.from_file.is_some() == self.link_to.is_some() {
                    bail!("symlink entry requires exactly one of 'from_file' and 'link_to'");
                }
                if self
                    .link_to
                    .as_ref()
                    .is_some_and(|link| link.is_empty() || link.contains('\0'))
                {
                    bail!("'link_to' must be non-empty and contain no NUL bytes");
                }
                if self.mode.is_some()
                    || (self.kind.is_some()
                        && (self.immutable
                            || self.local.is_some()
                            || !self.exclude.is_empty()
                            || self.format.is_some()
                            || !self.ignore.is_empty()))
                {
                    bail!(
                        "symlink entries cannot use mode, immutable, local, exclude, format, or ignore"
                    );
                }
            }
            ResourceKind::Directory => {
                if self.from_file.is_some()
                    || self.from_config.is_some()
                    || self.link_to.is_some()
                    || self.local.is_some()
                    || !self.exclude.is_empty()
                    || self.immutable
                    || self.format.is_some()
                    || !self.ignore.is_empty()
                {
                    bail!(
                        "directory entries manage only existence and optional mode; content inputs and options are invalid"
                    );
                }
            }
        }
        if self.local.is_some() && self.from_file.is_none() {
            bail!("'local' can only be used with 'from_file', not 'from_config'");
        }
        if let Some(mode) = &self.mode {
            parse_file_mode(mode)?;
        }
        if self.name.is_empty() {
            if let Some(from_file) = &self.from_file {
                self.name.clone_from(from_file);
            } else {
                bail!("'name' is required without 'from_file'");
            }
        }
        Ok(())
    }

    pub fn effective_kind(&self) -> ResourceKind {
        self.kind.unwrap_or(if self.symlink {
            ResourceKind::Symlink
        } else {
            ResourceKind::Content
        })
    }

    /// Parse the optional canonical four-digit octal file mode.
    pub fn parsed_mode(&self) -> Result<Option<u32>> {
        self.mode.as_deref().map(parse_file_mode).transpose()
    }

    /// Get the effective format (explicit or inferred from name)
    pub fn effective_format(&self) -> Format {
        self.format.unwrap_or_else(|| Format::from_path(&self.name))
    }

    /// Generate all target paths by combining prefix + name
    /// Uses file-level prefix if set, otherwise falls back to global prefix
    pub fn target_paths(&self, global_prefix: &[String]) -> Vec<PathBuf> {
        let prefixes = if self.prefix.is_empty() {
            global_prefix
        } else {
            &self.prefix
        };

        prefixes
            .iter()
            .map(|p| {
                let mut path = PathBuf::from(p);
                path.push(&self.name);
                path
            })
            .collect()
    }

    /// Check if this entry should be applied for the given system
    pub fn should_apply(&self, os: &str, arch: &str, hostname: &str) -> bool {
        match &self.when {
            Some(when) => when.matches(os, arch, hostname),
            None => true,
        }
    }
}

fn parse_file_mode(mode: &str) -> Result<u32> {
    if mode.len() != 4
        || !mode.starts_with('0')
        || !mode
            .as_bytes()
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'7'))
    {
        bail!("'mode' must be a four-digit octal string such as \"0600\"");
    }
    u32::from_str_radix(mode, 8).map_err(Into::into)
}

/// Metadata section of order.ncl (new multi-file schema)
#[derive(Debug, Clone, Deserialize)]
pub struct OrderMeta {
    /// Array of file entries
    #[serde(default)]
    pub files: Vec<FileEntry>,
    /// Default prefix for all files (can be overridden per-file)
    #[serde(default)]
    pub prefix: Vec<String>,
    /// Global condition for when to apply this order
    pub when: Option<WhenCondition>,
    /// Global keys to ignore when diffing
    #[serde(default)]
    pub ignore: Vec<String>,
}

/// Full order.ncl order structure (new schema)
#[derive(Debug, Clone, Deserialize)]
pub struct Order {
    /// Order metadata
    pub blend: OrderMeta,
    /// Per-entry marker produced by the Rust evaluation wrapper. This isn't
    /// part of the authored Order contract.
    #[serde(rename = "__blend_dynamic_entries", default)]
    pub(crate) dynamic_entries: Vec<bool>,
}

impl Order {
    /// Check if this order should be applied for the given system
    pub fn should_apply(&self, os: &str, arch: &str, hostname: &str) -> bool {
        match &self.blend.when {
            Some(when) => when.matches(os, arch, hostname),
            None => true,
        }
    }

    /// Get the global prefix
    pub fn global_prefix(&self) -> &[String] {
        &self.blend.prefix
    }

    /// Get the global ignore keys
    pub fn global_ignore(&self) -> &[String] {
        &self.blend.ignore
    }

    /// Check if this order applies on a given platform (os + arch), ignoring hostname
    pub fn applies_on_platform(&self, os: &str, arch: &str) -> bool {
        match &self.blend.when {
            Some(when) => when.matches_platform(os, arch),
            None => true,
        }
    }

    pub(crate) fn entry_has_resolution_semantics(&self, index: usize) -> bool {
        self.dynamic_entries.get(index).copied().unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_mode_requires_canonical_octal_string() {
        assert_eq!(parse_file_mode("0600").unwrap(), 0o600);
        for invalid in ["600", "00600", "0800", "0o600", "0644 "] {
            assert!(
                parse_file_mode(invalid).is_err(),
                "accepted invalid mode {invalid}"
            );
        }
    }

    #[test]
    fn test_format_from_path() {
        assert_eq!(Format::from_path("~/.config/starship.toml"), Format::Toml);
        assert_eq!(Format::from_path("settings.json"), Format::Json);
        assert_eq!(Format::from_path("config.yaml"), Format::Yaml);
        assert_eq!(Format::from_path("config.yml"), Format::Yaml);
        assert_eq!(Format::from_path("kitty.conf"), Format::Plaintext);
        assert_eq!(Format::from_path("init.lua"), Format::Plaintext);
    }

    #[test]
    fn test_when_condition_matches() {
        let when = WhenCondition {
            os: vec!["darwin".to_string(), "linux".to_string()],
            arch: vec![],
            hostname: vec![],
        };

        assert!(when.matches("darwin", "aarch64", "myhost"));
        assert!(when.matches("linux", "x86_64", "myhost"));
        assert!(!when.matches("windows", "x86_64", "myhost"));
    }

    #[test]
    fn test_when_condition_empty() {
        let when = WhenCondition::default();
        assert!(when.matches("darwin", "aarch64", "myhost"));
    }

    #[test]
    fn test_file_entry_target_paths_with_local_prefix() {
        let entry = FileEntry {
            kind: None,
            link_to: None,
            name: "settings.json".to_string(),
            from_file: None,
            from_config: Some(serde_json::json!({})),
            prefix: vec!["~/A/".to_string(), "~/B/".to_string()],
            format: None,
            ignore: vec![],
            when: None,
            symlink: false,
            exclude: vec![],
            local: None,
            immutable: false,
            mode: None,
        };

        // Local prefix takes precedence over global
        let global_prefix = vec!["~/global/".to_string()];
        let paths = entry.target_paths(&global_prefix);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], PathBuf::from("~/A/settings.json"));
        assert_eq!(paths[1], PathBuf::from("~/B/settings.json"));
    }

    #[test]
    fn test_file_entry_target_paths_with_global_prefix() {
        let entry = FileEntry {
            kind: None,
            link_to: None,
            name: "settings.json".to_string(),
            from_file: None,
            from_config: Some(serde_json::json!({})),
            prefix: vec![], // Empty - should use global
            format: None,
            ignore: vec![],
            when: None,
            symlink: false,
            exclude: vec![],
            local: None,
            immutable: false,
            mode: None,
        };

        let global_prefix = vec!["~/global/".to_string()];
        let paths = entry.target_paths(&global_prefix);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], PathBuf::from("~/global/settings.json"));
    }

    #[test]
    fn test_resolve_defaults_from_file() {
        let mut entry = FileEntry {
            kind: None,
            link_to: None,
            name: String::new(),
            from_file: Some("nvim".to_string()),
            from_config: None,
            prefix: vec![],
            format: None,
            ignore: vec![],
            when: None,
            symlink: false,
            exclude: vec![],
            local: None,
            immutable: false,
            mode: None,
        };
        entry.resolve_defaults().unwrap();
        assert_eq!(entry.name, "nvim");
    }

    #[test]
    fn test_resolve_defaults_from_config_requires_name() {
        let mut entry = FileEntry {
            kind: None,
            link_to: None,
            name: String::new(),
            from_file: None,
            from_config: Some(serde_json::json!({})),
            prefix: vec![],
            format: None,
            ignore: vec![],
            when: None,
            symlink: false,
            exclude: vec![],
            local: None,
            immutable: false,
            mode: None,
        };
        assert!(entry.resolve_defaults().is_err());
    }

    #[test]
    fn test_resolve_defaults_both_set_errors() {
        let mut entry = FileEntry {
            kind: None,
            link_to: None,
            name: "test".to_string(),
            from_file: Some("test".to_string()),
            from_config: Some(serde_json::json!({})),
            prefix: vec![],
            format: None,
            ignore: vec![],
            when: None,
            symlink: false,
            exclude: vec![],
            local: None,
            immutable: false,
            mode: None,
        };
        assert!(entry.resolve_defaults().is_err());
    }

    #[test]
    fn test_resolve_defaults_neither_set_errors() {
        let mut entry = FileEntry {
            kind: None,
            link_to: None,
            name: "test".to_string(),
            from_file: None,
            from_config: None,
            prefix: vec![],
            format: None,
            ignore: vec![],
            when: None,
            symlink: false,
            exclude: vec![],
            local: None,
            immutable: false,
            mode: None,
        };
        assert!(entry.resolve_defaults().is_err());
    }

    #[test]
    fn test_resolve_defaults_symlink_with_from_config_errors() {
        let mut entry = FileEntry {
            kind: None,
            link_to: None,
            name: "test".to_string(),
            from_file: None,
            from_config: Some(serde_json::json!({})),
            prefix: vec![],
            format: None,
            ignore: vec![],
            when: None,
            symlink: true,
            exclude: vec![],
            local: None,
            immutable: false,
            mode: None,
        };
        assert!(entry.resolve_defaults().is_err());
    }

    #[test]
    fn test_resolve_defaults_symlink_with_from_file_ok() {
        let mut entry = FileEntry {
            kind: None,
            link_to: None,
            name: String::new(),
            from_file: Some("bin".to_string()),
            from_config: None,
            prefix: vec![],
            format: None,
            ignore: vec![],
            when: None,
            symlink: true,
            exclude: vec![],
            local: None,
            immutable: false,
            mode: None,
        };
        entry.resolve_defaults().unwrap();
        assert_eq!(entry.name, "bin");
    }

    #[test]
    fn test_resolve_defaults_local_with_from_config_errors() {
        let mut entry = FileEntry {
            kind: None,
            link_to: None,
            name: "test".to_string(),
            from_file: None,
            from_config: Some(serde_json::json!({})),
            prefix: vec![],
            format: None,
            ignore: vec![],
            when: None,
            symlink: false,
            exclude: vec![],
            local: Some("test.local".to_string()),
            immutable: false,
            mode: None,
        };
        assert!(entry.resolve_defaults().is_err());
    }

    #[test]
    fn test_resolve_defaults_local_with_from_file_ok() {
        let mut entry = FileEntry {
            kind: None,
            link_to: None,
            name: String::new(),
            from_file: Some("elvish".to_string()),
            from_config: None,
            prefix: vec![],
            format: None,
            ignore: vec![],
            when: None,
            symlink: false,
            exclude: vec![],
            local: Some("elvish.local".to_string()),
            immutable: false,
            mode: None,
        };
        entry.resolve_defaults().unwrap();
        assert_eq!(entry.name, "elvish");
    }
}
