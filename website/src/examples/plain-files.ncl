files = [
  {
    from_file = "snippets",
    exclude = ["*.tmp"],
    local = "snippets.local",
  },
  {
    name = "van",
    from_file = "bin",
    symlink = true,
  },
],
