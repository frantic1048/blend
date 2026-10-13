let { Order, .. } = import "../order.contract.ncl" in
let metadata = import "../metadata.ncl" in
{
  blend = {
    prefix = ["~/.config/editor/"],
    files = [
      {
        name = "settings.toml",
        from_config = {
          theme = "latte",
          font_size =
            if metadata.os == "darwin" then 14 else 12,
          plugins = ["git", "lsp"],
        },
      },
    ],
  },
} | Order
