# cuemix-web container image (linux/amd64, linux/arm64).
#
# This Dockerfile does NOT compile anything. It expects prebuilt static musl
# binaries placed in the build context by .github/workflows/release.yml:
#   dist/amd64/cuemix-web   (x86_64-unknown-linux-musl)
#   dist/arm64/cuemix-web   (aarch64-unknown-linux-musl)
#
# Build locally:
#   make frontend
#   cargo zigbuild --release --locked --target x86_64-unknown-linux-musl
#   cargo zigbuild --release --locked --target aarch64-unknown-linux-musl
#   mkdir -p dist/amd64 dist/arm64
#   cp target/x86_64-unknown-linux-musl/release/cuemix-web  dist/amd64/
#   cp target/aarch64-unknown-linux-musl/release/cuemix-web dist/arm64/
#   docker buildx build -f deploy/Dockerfile --platform linux/amd64,linux/arm64 .
# Pinned by digest (multi-arch index); Dependabot proposes new digests.
FROM gcr.io/distroless/static-debian12:nonroot@sha256:afa5c872c891853ca7fcf1f12c3edb23f7eeef36189728842dd51042ff57f7ab

ARG TARGETARCH
COPY --chmod=0755 dist/${TARGETARCH}/cuemix-web /usr/local/bin/cuemix-web

VOLUME ["/etc/cuemix-web", "/var/lib/cuemix-web"]
EXPOSE 8080/tcp 8000/udp

USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/cuemix-web", "--config", "/etc/cuemix-web/config.toml"]
