FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2

LABEL org.opencontainers.image.source="https://github.com/frantic1048/blend"
LABEL org.opencontainers.image.title="blend"
LABEL org.opencontainers.image.description="blend: dotfiles manager with Nickel DSL"
LABEL org.opencontainers.image.url="https://github.com/frantic1048/blend"
LABEL org.opencontainers.image.documentation="https://github.com/frantic1048/blend#readme"
LABEL org.opencontainers.image.licenses="MIT"

COPY blend /usr/local/bin/blend
ENTRYPOINT ["/usr/local/bin/blend"]
