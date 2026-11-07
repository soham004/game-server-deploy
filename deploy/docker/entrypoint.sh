#!/bin/sh
# Container entrypoint.
#
# Applies container defaults (data in /data, listen on all interfaces inside the container),
# then runs the app. The binary name, port and env-var prefix come from /etc/app-brand, which the
# Docker build writes from the binary itself, so nothing here is hard-coded.
set -eu

# shellcheck disable=SC1091
. /etc/app-brand # APP_SLUG, APP_PORT, APP_ENV_PREFIX

# Defaults only: values set in the compose file (e.g. <PREFIX>_BIND) win.
eval ": \"\${${APP_ENV_PREFIX}_DATA_DIR:=/data}\""
eval ": \"\${${APP_ENV_PREFIX}_BIND:=0.0.0.0:${APP_PORT}}\""
export "${APP_ENV_PREFIX}_DATA_DIR" "${APP_ENV_PREFIX}_BIND"

exec "/usr/local/bin/${APP_SLUG}" "$@"
