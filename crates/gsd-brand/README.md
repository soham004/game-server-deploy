# gsd-brand

The only place the product name, slug and default port are defined. They are set at build time:

| Variable | Default | Notes |
|---|---|---|
| `APP_NAME` | see `build.rs` | Display name |
| `APP_SLUG` | see `build.rs` | `[a-z][a-z0-9-]{1,31}`; drives the env var prefix, cookie name and data dir |
| `APP_DEFAULT_PORT` | see `build.rs` | Default web port; runtime config can override it |

Everything else uses `gsd_brand::{NAME, SLUG, DEFAULT_PORT, ENV_PREFIX, env_var(), user_agent()}`.
