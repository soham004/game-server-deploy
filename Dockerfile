# syntax=docker/dockerfile:1
#
# Linux image: one container runs the app and every game server it manages (D-014, D-016).
#
# Branding is a build-time setting (D-017). Empty build args mean "use the defaults declared in
# crates/gsd-brand/build.rs", so no name, slug or port is written in this file:
#
#   docker build --build-arg APP_NAME="My Panel" --build-arg APP_SLUG=mypanel -t mypanel .

ARG APP_NAME=""
ARG APP_SLUG=""
ARG APP_DEFAULT_PORT=""

FROM rust:1-slim-trixie AS build
ARG APP_NAME
ARG APP_SLUG
ARG APP_DEFAULT_PORT
ENV APP_NAME=${APP_NAME} APP_SLUG=${APP_SLUG} APP_DEFAULT_PORT=${APP_DEFAULT_PORT}
WORKDIR /src
COPY . .
# The target dir is a cache mount, so the binary and its branding facts are copied to /out in
# the same step.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p gsd-app \
 && bin=target/release/gsd \
 && slug="$($bin brand slug)" \
 && install -D "$bin" "/out/usr/local/bin/$slug" \
 && mkdir -p /out/etc \
 && printf 'APP_SLUG=%s\nAPP_PORT=%s\nAPP_ENV_PREFIX=%s\n' \
      "$slug" "$($bin brand port)" "$($bin brand env-prefix)" > /out/etc/app-brand

FROM debian:trixie-slim
# ca-certificates: HTTPS to Mojang/Modrinth/Adoptium. tini: reaps orphaned grandchildren
# (installers, JVM helpers). tzdata: schedules run in the admin's timezone.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tini tzdata \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/ /
COPY deploy/docker/entrypoint.sh /usr/local/lib/app/entrypoint.sh
RUN chmod 0755 /usr/local/lib/app/entrypoint.sh

VOLUME ["/data"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/lib/app/entrypoint.sh"]
CMD ["serve"]
HEALTHCHECK --interval=30s --timeout=10s --start-period=20s --retries=3 \
  CMD ["/usr/local/lib/app/entrypoint.sh", "healthcheck"]
