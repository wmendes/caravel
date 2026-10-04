# syntax=docker/dockerfile:1
# The relayer and every template's feed modules, taken from an assembled
# release with their production node_modules (DEC-111). The context is the
# release directory.
#   docker build -f docker/relayer.Dockerfile <release>
FROM node:22.23.3-trixie-slim@sha256:b26b04c123d9ff8ab646ceb18b9d75a1173acf64b9a401094b906d27b29338d4
ARG COMMIT=unknown
LABEL org.opencontainers.image.source="https://github.com/wmendes/caravel" \
      org.opencontainers.image.description="Caravel relayer: inbox, checkpoints and feed modules" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0" \
      org.opencontainers.image.revision="${COMMIT}"
RUN groupadd -g 10001 caravel && useradd -u 10001 -g 10001 -M -d /opt/caravel -s /usr/sbin/nologin caravel
COPY relayer /opt/caravel/relayer
COPY relayer-feeds /opt/caravel/relayer-feeds
COPY COMMIT /opt/caravel/COMMIT
USER 10001:10001
WORKDIR /opt/caravel/relayer
ENTRYPOINT ["node", "/opt/caravel/relayer/dist/main.js"]
