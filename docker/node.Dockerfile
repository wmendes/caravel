# syntax=docker/dockerfile:1
# A lane's node: its template's binary and engine Wasm, taken from an
# assembled release (scripts/assemble-release.sh), not rebuilt (DEC-111).
# The context is the release directory.
#   docker build -f docker/node.Dockerfile --build-arg TEMPLATE=perps <release>
FROM debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a
ARG TEMPLATE
ARG COMMIT=unknown
LABEL org.opencontainers.image.source="https://github.com/wmendes/caravel" \
      org.opencontainers.image.description="Caravel ${TEMPLATE} lane node (sequencer, validator, replay)" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0" \
      org.opencontainers.image.revision="${COMMIT}"
# ca-certificates: the node calls Stellar RPC and validators over TLS (rustls, native roots).
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd -g 10001 caravel \
 && useradd -u 10001 -g 10001 -M -d /opt/caravel -s /usr/sbin/nologin caravel
COPY --chmod=755 bin/caravel-${TEMPLATE}-node /opt/caravel/bin/
COPY contracts/${TEMPLATE}_engine.wasm /opt/caravel/contracts/
COPY COMMIT /opt/caravel/COMMIT
RUN ln -s caravel-${TEMPLATE}-node /opt/caravel/bin/caravel-node
USER 10001:10001
WORKDIR /opt/caravel
ENTRYPOINT ["/opt/caravel/bin/caravel-node"]
CMD ["--help"]
