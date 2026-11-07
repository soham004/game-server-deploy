# Plan 07: Deployment (Linux-first, Docker) and Branding

## User requirements (2026-11-07)
- **Linux first**, packaged as a **Docker image** for easy deployment anywhere. Windows is a secondary, development or home target.
- **Browser-only** UI (no desktop wrapper).
- Remote HTTPS through an **nginx** reverse proxy (documented, with an example). The app itself serves plain HTTP.
- **Product name and default port configurable at build time**, declared in exactly one place and referenced everywhere else.

## 1. Branding: one source of truth

### Build-time inputs (environment variables read by a build script)
| Var | Default | Used for |
|---|---|---|
| `APP_NAME` | `Game Server Deploy` | UI title, logs, User-Agent product name, OpenAPI title |
| `APP_SLUG` | `gsd` | data dir name, env var prefix (`GSD_…`), cookie name, systemd unit, Docker image/binary name |
| `APP_DEFAULT_PORT` | `8737` | default web port (runtime config can still override) |

### Where it is declared
A tiny crate **`crates/gsd-brand`** is the *only* place these values exist:
```
crates/gsd-brand/
├─ build.rs      # reads APP_NAME / APP_SLUG / APP_DEFAULT_PORT (cargo:rerun-if-env-changed=…),
│                # validates (slug = [a-z][a-z0-9-]{1,31}, port 1–65535), writes $OUT_DIR/brand.rs
└─ src/lib.rs    # include!(concat!(env!("OUT_DIR"), "/brand.rs"));
```
Generated API (consts + derived helpers):
```rust
pub const NAME: &str = "Game Server Deploy";
pub const SLUG: &str = "gsd";
pub const DEFAULT_PORT: u16 = 8737;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const ENV_PREFIX: &str = "GSD";                 // derived: SLUG upper-cased, '-' → '_'
pub fn env_var(name: &str) -> String { format!("{ENV_PREFIX}_{name}") }   // env_var("DATA_DIR") → "GSD_DATA_DIR"
pub fn user_agent() -> String { format!("{SLUG}/{VERSION} (+<repo url>)") }
pub const SESSION_COOKIE: &str = "gsd_session";     // derived at build time
```
Rules:
- **No other crate, file or test may hard-code the name, slug or port.** They use `gsd_brand::*`. A CI grep check fails the build
  if the default literals (`8737`, `"Game Server Deploy"`) appear outside `gsd-brand`.
- Internal crate names (`gsd-core`, …) are developer-facing and stay as they are. Changing them is a cheap rename, not branding.
- The binary name in `crates/gsd-app/Cargo.toml` (`[[bin]] name = "gsd"`) is the one Cargo-imposed exception. The Dockerfile copies
  it to `/usr/local/bin/${APP_SLUG}`.
- **Frontend**: `web/` never contains the name. `index.html` uses `{{APP_NAME}}` placeholders that the asset handler fills in once
  at startup (cached), and JS reads `GET /api/v1/meta` → `{ name, slug, version }` for anything dynamic (page titles, about box).
- Runtime port precedence: CLI `--port` > env `<PREFIX>_PORT` > `config.toml` > `gsd_brand::DEFAULT_PORT`.

## 2. Docker image
```dockerfile
# syntax=docker/dockerfile:1
ARG APP_NAME="Game Server Deploy" APP_SLUG=gsd APP_DEFAULT_PORT=8737

FROM rust:1-slim-trixie AS build            # pin the exact version at implementation time
ARG APP_NAME APP_SLUG APP_DEFAULT_PORT
ENV APP_NAME=$APP_NAME APP_SLUG=$APP_SLUG APP_DEFAULT_PORT=$APP_DEFAULT_PORT
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p gsd-app

FROM debian:trixie-slim
ARG APP_SLUG APP_DEFAULT_PORT
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates tini tzdata \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/gsd /usr/local/bin/${APP_SLUG}
# entrypoint script resolves the slug-named binary; tini reaps orphaned grandchildren (installers, JVM forks)
ENTRYPOINT ["/usr/bin/tini","--","/usr/local/bin/entrypoint.sh"]
VOLUME ["/data"]
EXPOSE ${APP_DEFAULT_PORT}
HEALTHCHECK CMD ["/usr/local/bin/entrypoint.sh","healthcheck"]
```
- Glibc base (Debian) because the Temurin downloads are glibc builds. Java runtimes are **downloaded on demand** into
  `/data/runtimes` (persisted). Optional image variant `:<ver>-java` with Temurin 21 and 25 preinstalled for offline hosts.
- Inside the container: data dir `/data`, bind `0.0.0.0` (host exposure is controlled by the port mapping). The process runs as
  root **inside** the container with Docker's default capabilities, dropped to `SETUID, SETGID, CHOWN, DAC_OVERRIDE, FOWNER, KILL`
  in the compose file. Each Minecraft server runs as its own UID (see `features/isolation-and-sandboxing.md`).
- Multi-arch builds: `linux/amd64` and `linux/arm64` (Raspberry Pi 4/5) via `docker buildx`.
- **Game ports**: Docker needs ports declared up front, so the app has a configured **game port range** (`<PREFIX>_GAME_PORTS=25565-25584`,
  20 servers by default per D-022) and the instance port allocator only hands out ports from it. Compose maps the same range.
  RCON and MCSMP ports come from a separate internal loopback range (`<PREFIX>_INTERNAL_PORTS=35565-35664`) and are never published. Alternative documented:
  `network_mode: host` (Linux only; simplest, no range needed).

### docker-compose example (with nginx TLS)
```yaml
services:
  app:
    image: ghcr.io/<owner>/gsd:latest
    restart: unless-stopped
    cap_drop: [ALL]
    cap_add: [SETUID, SETGID, CHOWN, DAC_OVERRIDE, FOWNER, KILL]
    environment:
      GSD_GAME_PORTS: "25565-25584"
      GSD_TRUSTED_PROXIES: "172.16.0.0/12"
    volumes: ["./data:/data"]
    ports: ["25565-25584:25565-25584"]        # game ports; the web UI is reached only through nginx
  nginx:
    image: nginx:stable
    restart: unless-stopped
    ports: ["443:443", "80:80"]
    volumes:
      - ./nginx/gsd.conf:/etc/nginx/conf.d/default.conf:ro
      - ./certs:/etc/nginx/certs:ro           # or a certbot/acme companion
```
### nginx site config (documented in `docs/deploy/nginx.md` later)
```nginx
server { listen 80; server_name panel.example.com; return 301 https://$host$request_uri; }
server {
  listen 443 ssl; http2 on; server_name panel.example.com;
  ssl_certificate /etc/nginx/certs/fullchain.pem; ssl_certificate_key /etc/nginx/certs/privkey.pem;
  add_header Strict-Transport-Security "max-age=31536000" always;
  client_max_body_size 4g;                    # world/modpack uploads
  location / {
    proxy_pass http://app:8737;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
    proxy_set_header Upgrade $http_upgrade;   # WebSocket
    proxy_set_header Connection $connection_upgrade;
    proxy_read_timeout 1h;                    # long-lived WS + big downloads
    proxy_request_buffering off;              # stream uploads
  }
}
map $http_upgrade $connection_upgrade { default upgrade; '' close; }
```
- The app trusts `X-Forwarded-*` **only** from `TRUSTED_PROXIES` CIDRs. It marks cookies `Secure` when the forwarded proto is https.

## 3. Bare-metal Linux (systemd)
`<slug> service install` writes `/etc/systemd/system/<slug>.service`:
```ini
[Service]
ExecStart=/usr/local/bin/gsd serve --data-dir /var/lib/gsd
Restart=on-failure
KillMode=mixed            # app stops servers gracefully itself; systemd only kills leftovers
TimeoutStopSec=180
Delegate=yes              # lets the app create per-instance cgroups (memory.max, pids.max)
NoNewPrivileges=yes       # fine: setuid() from root needs no exec privilege gain
ProtectSystem=strict
ReadWritePaths=/var/lib/gsd
PrivateTmp=yes
CapabilityBoundingSet=CAP_SETUID CAP_SETGID CAP_CHOWN CAP_DAC_OVERRIDE CAP_FOWNER CAP_KILL
```
(The literal `gsd` above is illustrative. The real unit is generated from `gsd_brand::SLUG`.)

## 4. Windows / macOS
Supported for **development** and simple single-user home use: plain processes (a Job Object on Windows), isolation level `None`,
shown in the UI. No installer in v1. A zip release can come later.

## 5. CI / release
- CI matrix priority: **Linux x64 (required)**, Linux arm64 (required for images), Windows (build + unit tests), macOS (build only).
- Integration tests for the sandbox run on Linux only (they need a kernel with Landlock; skip gracefully when it is absent).
- Release: GitHub Actions builds multi-arch images (`ghcr.io`) and static-ish Linux tarballs. Branding build args are exposed in the
  workflow so a rebranded build is a one-line change.
