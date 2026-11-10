# The relay on workerd, for running it yourself (docs/relay-self-host.md). The same code as the Cloudflare relay; rooms live in /data.
# Build in relay/:   docker build -t kelpie-relay .
# Run:               docker run -d -p 8787:8787 -v kelpie-relay:/data -e TICKET_SECRET -e REGISTRATION_KEY kelpie-relay
# Registration: with REGISTRATION_KEY set, only its holder makes rooms. Without it, -e OPEN_REGISTRATION=1 lets anyone make them,
# ROOMS_PER_NETWORK_DAY (20) per network and ROOMS_PER_DAY (1000) in all per UTC day; with neither, no rooms are made.
# Behind your own proxy every client counts as one network. This relay serves no phone app (config.capnp).
# Put a TLS proxy in front (Caddy, say): phones and Kelpie connect with wss://.
FROM node:24-bookworm-slim AS build
WORKDIR /relay
RUN corepack enable
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY src ./src
COPY workerd ./workerd
RUN pnpm build:selfhost \
 && cp node_modules/.pnpm/@cloudflare+workerd-linux-64@*/node_modules/@cloudflare/workerd-linux-64/bin/workerd /relay/workerd-bin

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 relay && mkdir -p /relay/dist /data && chown relay /data
COPY --from=build /relay/workerd-bin /usr/local/bin/workerd
COPY --from=build /relay/workerd/config.capnp /relay/config.capnp
COPY --from=build /relay/workerd/dist/entry.js /relay/dist/entry.js
# config.capnp keeps rooms in ./rooms: here that is the volume.
RUN ln -s /data /relay/rooms
USER relay
WORKDIR /relay
VOLUME /data
EXPOSE 8787
CMD ["workerd", "serve", "config.capnp"]
