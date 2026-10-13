mode =
  fun { target } =>
    target
    |> match {
      "legacy" => "modern",
      "modern" => target,
      _ => 'Unresolved,
    },
