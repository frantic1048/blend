FROM debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a

LABEL org.opencontainers.image.source="https://github.com/frantic1048/blend"
LABEL org.opencontainers.image.title="blend"
LABEL org.opencontainers.image.description="blend: dotfiles manager with Nickel DSL"
LABEL org.opencontainers.image.url="https://github.com/frantic1048/blend"
LABEL org.opencontainers.image.documentation="https://github.com/frantic1048/blend#readme"
LABEL org.opencontainers.image.licenses="MIT"

COPY blend /usr/local/bin/blend
ENTRYPOINT ["/usr/local/bin/blend"]
