FROM debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4

LABEL org.opencontainers.image.source="https://github.com/frantic1048/blend"
LABEL org.opencontainers.image.title="blend"
LABEL org.opencontainers.image.description="blend: dotfiles manager with Nickel DSL"
LABEL org.opencontainers.image.url="https://github.com/frantic1048/blend"
LABEL org.opencontainers.image.documentation="https://github.com/frantic1048/blend#readme"
LABEL org.opencontainers.image.licenses="MIT"

COPY blend /usr/local/bin/blend
ENTRYPOINT ["/usr/local/bin/blend"]
