# The relay on workerd, Cloudflare's open-source runtime: the same code as on Cloudflare, with each room's SQLite on local disk.
# `pnpm build:selfhost` makes dist/entry.js. Settings come from the environment and are never written here:
#   TICKET_SECRET          signs room tickets (32 random bytes, base64, say)
#   REGISTRATION_KEY       what Kelpie needs to make a room; when set, the only way to make one
#   OPEN_REGISTRATION      "1": with no REGISTRATION_KEY, anyone may make rooms, within the limits below
#   ROOMS_PER_NETWORK_DAY  rooms one network may make in a UTC day without a key (20 when unset)
#   ROOMS_PER_DAY          rooms everyone together may make in a UTC day without a key (1000 when unset)
# A network is the client's address as Cloudflare gives it (cf-connecting-ip); behind your own proxy every client is one network.
# This relay serves no phone app: its computers' phones use Tailscale.
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
  durableObjectNamespaces = [
    (className = "Room", uniqueKey = "kelpie-link-relay-room", enableSql = true),
    (className = "Registrar", uniqueKey = "kelpie-link-relay-registrar", enableSql = true),
  ],
  durableObjectStorage = (localDisk = "rooms"),
  bindings = [
    (name = "ROOM", durableObjectNamespace = "Room"),
    (name = "REGISTRAR", durableObjectNamespace = "Registrar"),
    (name = "TICKET_SECRET", fromEnvironment = "TICKET_SECRET"),
    (name = "REGISTRATION_KEY", fromEnvironment = "REGISTRATION_KEY"),
    (name = "OPEN_REGISTRATION", fromEnvironment = "OPEN_REGISTRATION"),
    (name = "ROOMS_PER_NETWORK_DAY", fromEnvironment = "ROOMS_PER_NETWORK_DAY"),
    (name = "ROOMS_PER_DAY", fromEnvironment = "ROOMS_PER_DAY"),
    (name = "RELAY_VERSION", text = "2"),
  ],
);
