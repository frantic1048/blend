let { Order, .. } = import "../order.contract.ncl" in
{
  blend = {
    prefix = ["~/.config/editor/"],
    files = [
      {
        name = "settings.toml",
        format = "toml",
        from_config =
          {
            theme = 'Enforce "dark",
          }
          |> blend.with_target_only 'Unmanaged,
      },
    ],
  },
} | Order
