# syntax=docker/dockerfile:1
# Caddy with a template's web app, taken from an assembled release (DEC-111).
# The Caddyfile and the deployment's web.json are mounted at run time, as
# `caravel apply` renders them. The context is the release directory.
#   docker build -f docker/web.Dockerfile --build-arg TEMPLATE=perps <release>
FROM caddy:2.11.6@sha256:907efba736324e43f891ccb9d760fe5abe545e313419b3d18d63d4ec670dad8d
ARG TEMPLATE
ARG COMMIT=unknown
LABEL org.opencontainers.image.source="https://github.com/wmendes/caravel" \
      org.opencontainers.image.description="Caravel ${TEMPLATE} web app, served by Caddy" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0" \
      org.opencontainers.image.revision="${COMMIT}"
COPY web/${TEMPLATE} /opt/caravel/web
COPY COMMIT /opt/caravel/COMMIT
