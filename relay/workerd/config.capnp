# The relay on workerd, Cloudflare's open-source runtime: the same code as on Cloudflare, with each room's SQLite on local disk.
# `pnpm build:selfhost` makes dist/entry.js. Secrets come from the environment and are never written here:
#   TICKET_SECRET     signs room tickets (32 random bytes, base64, say)
#   REGISTRATION_KEY  what Kelpie needs to make a room
# Run: workerd serve config.capnp   (rooms in ./rooms unless KELPIE_RELAY_DATA is mounted there; see docs/relay-self-host.md).
using Workerd = import "/workerd/workerd.capnp";

const config :Workerd.Config = (
  services = [
    (name = "relay", worker = .relay),
    (name = "rooms", disk = (path = "rooms", writable = true)),
  ],
  sockets = [(name = "http", address = "*:8787", http = (), service = "relay")],
);

const relay :Workerd.Worker = (
  modules = [(name = "entry.js", esModule = embed "dist/entry.js")],
  compatibilityDate = "2026-10-15",
  durableObjectNamespaces = [(className = "Room", uniqueKey = "kelpie-link-relay-room", enableSql = true)],
  durableObjectStorage = (localDisk = "rooms"),
  bindings = [
    (name = "ROOM", durableObjectNamespace = "Room"),
    (name = "TICKET_SECRET", fromEnvironment = "TICKET_SECRET"),
    (name = "REGISTRATION_KEY", fromEnvironment = "REGISTRATION_KEY"),
    (name = "RELAY_VERSION", text = "1"),
  ],
);
