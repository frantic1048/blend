FROM debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f

LABEL org.opencontainers.image.source="https://github.com/frantic1048/blend"
LABEL org.opencontainers.image.title="blend"
LABEL org.opencontainers.image.description="blend: dotfiles manager with Nickel DSL"
LABEL org.opencontainers.image.url="https://github.com/frantic1048/blend"
LABEL org.opencontainers.image.documentation="https://github.com/frantic1048/blend#readme"
LABEL org.opencontainers.image.licenses="MIT"

COPY blend /usr/local/bin/blend
ENTRYPOINT ["/usr/local/bin/blend"]
