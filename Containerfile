# cost-report container image: one binary for the server, the snapshotter
# and the volume-usage collector (the Deployment picks the command).
#
# Built natively per architecture (linux/amd64 on x86-64 runners,
# linux/arm64 on ARM64 runners); no QEMU emulation, no cross-compiling.
#
#   docker buildx build -f Containerfile --build-arg GIT_SHA=$(git rev-parse HEAD) .
#
# Stages:
#   builder    SUSE BCI Rust: compiles the release binary.
#   micro      SUSE BCI Micro: the runtime base (glibc, CA certificates,
#              no package manager, no shell).
#   installer  SUSE BCI Base: adds libgcc_s1 (the Rust binary links
#              libgcc_s.so.1) and the non-root account into a copy of the
#              Micro root with `zypper --installroot`, the method SUSE
#              documents for adding packages to BCI Micro.
#   final      Micro plus the prepared root and the binary.

ARG BCI_RELEASE=15.7

FROM registry.suse.com/bci/rust:1.98 AS builder
ARG GIT_SHA=unknown
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# GIT_SHA is an environment variable during RUN, so option_env!("GIT_SHA")
# picks it up at compile time.
RUN grep PRETTY_NAME /etc/os-release \
 && cargo --version \
 && cargo build --release --locked -p cost-report \
 && install -D -m 0755 target/release/cost-report /out/cost-report \
 && { ldd /out/cost-report || true; }

FROM registry.suse.com/bci/bci-micro:${BCI_RELEASE} AS micro

FROM registry.suse.com/bci/bci-base:${BCI_RELEASE} AS installer
COPY --from=micro / /chroot/
# libgcc_s1 is deliberately not version-pinned: BCI_RELEASE pins the SLE
# release, and the package must match that release's glibc and gcc runtime.
# hadolint ignore=DL3037
RUN zypper --installroot /chroot -n --gpg-auto-import-keys install --no-recommends libgcc_s1 \
 && zypper --installroot /chroot clean -a \
 && rm -rf /chroot/var/log/ \
 && echo 'cost-report:x:65532:65532:cost-report:/nonexistent:/usr/sbin/nologin' >> /chroot/etc/passwd \
 && echo 'cost-report:x:65532:' >> /chroot/etc/group

FROM micro
ARG GIT_SHA=unknown
ARG VERSION=1.0.0
COPY --from=installer /chroot/ /
COPY --from=builder /out/cost-report /usr/local/bin/cost-report
# Fail the build here, not in the cluster, if a shared library or glibc
# symbol version the binary needs is missing from the runtime image.
RUN ["/usr/local/bin/cost-report", "--version"]

LABEL org.opencontainers.image.title="cost-report" \
      org.opencontainers.image.description="Department cost allocation from OpenCost: aggregated API, snapshotter, volume-usage collector" \
      org.opencontainers.image.source="https://github.com/yansheng133/opencost-dept-report" \
      org.opencontainers.image.url="https://github.com/yansheng133/opencost-dept-report" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.revision="${GIT_SHA}" \
      org.opencontainers.image.version="${VERSION}"

# Numeric UID:GID so Kubernetes runAsNonRoot can verify it without /etc/passwd.
USER 65532:65532
EXPOSE 8080 8443
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/cost-report"]
CMD ["server"]
