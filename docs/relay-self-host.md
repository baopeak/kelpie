# Running the relay yourself

Kelpie's phones can reach a computer through a relay (Settings → Mobile, *Reach this computer through the relay*). By default that is the
relay Kelpie's maintainer runs on Cloudflare, which also serves the phone app. The same code runs on
[workerd](https://github.com/cloudflare/workerd), Cloudflare's open-source runtime, so you can run your own: it sees what any relay sees
(that a phone and a computer are connected, their addresses, when and how much), and never what they say. The wire is in
[link-protocol.md](link-protocol.md).

A relay on workerd serves **no phone app**: the computers that use it pair their phones over Tailscale (the user guide's "Phone access"
has both ways). Serving the app needs Cloudflare's static assets, and makes the account that deploys them the phones' trust root: see
"Serving the phone app from your own Cloudflare account" below.

## What you need

- A machine reachable from the internet, with a name and a TLS certificate: phones and Kelpie connect with `wss://`, and the phone app's
  page is on HTTPS, so it cannot use a plain `ws://` relay.
- Docker, or Node 24 with pnpm 11 to build it yourself.
- `TICKET_SECRET`, which never goes in a file of the repository: it signs the rooms' tickets. 32 random bytes, for instance
  `openssl rand -base64 32`.
- A choice of who may make rooms (next section).

## Who may make a room

Making a room is `POST /v1/rooms` (the wire is in link-protocol.md). The relay decides in this order:

1. **`REGISTRATION_KEY` is set**: only its holder makes rooms. It wins over `OPEN_REGISTRATION`. Anyone who has the key can make rooms on
   your relay, so give it only to the computers you mean to serve. Setting it on a relay that was open closes it, without a deploy;
   rooms already made keep working.
2. **`OPEN_REGISTRATION=1`** and no key: anyone may, within the limits below.
3. **Neither**: no rooms are made (`503`, "relay not configured").

Open registration is what Kelpie's own relay uses. For a relay of your own a key is the safer default: the computers you serve
need it once, and nobody else can use your relay's room count or bandwidth.

Limits on rooms made without a key, counted per UTC day, each a variable you can set (a whole positive number):

| Variable | Default | Meaning |
| --- | --- | --- |
| `ROOMS_PER_NETWORK_DAY` | 20 | rooms one network may make |
| `ROOMS_PER_DAY` | 1000 | rooms everyone together may make |

A network is the client's address as `Cf-Connecting-Ip` gives it (an IPv6 address by its /64, an IPv4 address whole), hashed: no
address is stored. On Cloudflare a first, approximate shed also applies, 5 rooms a minute per network and location (the `ROOM_RATE`
binding in `relay/wrangler.toml`); workerd has none. A refusal says which limit it was in the `x-kelpie-reason` header (`rate`,
`network` or `full`); Kelpie turns that into words in Settings. With a key set, a network that sends 10 wrong keys within a day is answered `429` from then on.

## With Docker (one command)

Make the secrets once, in a file only you can read, and keep it: `TICKET_SECRET` must stay the same for as long as the relay lives
(tickets made with another one are refused, and every computer would have to make a new room). This example gives the relay a key; for an
open relay, write `OPEN_REGISTRATION=1` instead of the `REGISTRATION_KEY` line.

```bash
umask 077
printf 'TICKET_SECRET=%s\nREGISTRATION_KEY=%s\n' "$(openssl rand -base64 32)" "$(openssl rand -base64 32)" > /etc/kelpie-relay.env
cd relay
docker build -t kelpie-relay .
docker run -d --name kelpie-relay --restart unless-stopped -p 127.0.0.1:8787:8787 -v kelpie-relay:/data \
  --env-file /etc/kelpie-relay.env kelpie-relay
```

The rooms live in the `kelpie-relay` volume, so an upgrade (a new image, the same file and volume) keeps them. The secrets stay out of your
shell history; `docker inspect` still shows them to whoever may run Docker on that machine, so keep that to yourself.

## Without Docker

```bash
cd relay
pnpm install --frozen-lockfile
pnpm build:selfhost              # writes workerd/dist/entry.js
cd workerd
TICKET_SECRET=... REGISTRATION_KEY=... ../node_modules/.pnpm/@cloudflare+workerd-*/node_modules/@cloudflare/workerd-*/bin/workerd serve config.capnp
```

It listens on port 8787 and keeps rooms in `workerd/rooms`. `workerd/config.capnp` lists every variable it reads.

## TLS in front

Put a reverse proxy with a certificate in front, for instance [Caddy](https://caddyserver.com):

```
relay.example.com {
  reverse_proxy 127.0.0.1:8787 {
    header_up Cf-Connecting-Ip {remote_host}
  }
}
```

The relay counts wrong registration keys, rooms made without a key and refused sockets by a hash of `Cf-Connecting-Ip`, which Cloudflare sets
for it; behind your own proxy, set it as above, or every caller shares one count (one network, in open mode: its 20 rooms a day). What
phones send once they are in is limited per token and per room, not by address.

## Checking it

```bash
cd relay
RELAY_URL=https://relay.example.com REGISTRATION_KEY=... pnpm smoke
```

Leave `REGISTRATION_KEY` out for an open relay. It makes a room, signs in as a desktop, checks the protocol version and the page origins
the room takes, lets a phone in with a token, sends 100 frames each way, revokes the token and removes the room. `PAGE=1` also checks the
phone app served beside a relay on Cloudflare, which a relay on workerd does not have.

## Using it

In Kelpie, Settings → Mobile, *Reach this computer through the relay*: put `wss://relay.example.com` in *Relay address*, paste the
*Registration key* if the relay has one (Settings asks for it only when the relay says it needs one), and turn it on. The key is used once,
to make this computer's room, and kept in the computer's credential store for making a new room later. Under *Hosted phone app*, Settings
says that this relay does not serve the phone app; phones are paired over Tailscale and get their relay token the next time they connect.

## TURN for the screen (optional)

When a phone shows the computer's screen through the relay, Kelpie tries a direct connection between the phone and the computer (peer
to peer) and, where the relay offers TURN, a TURN-relayed one together, and keeps whichever works; your relay can hand out the short-lived TURN
credentials for it. Without TURN, such a screen stays on the relay's own connection while *Relay WebSocket* is on.

- **coturn**: run a [coturn](https://github.com/coturn/coturn) server with `use-auth-secret` and a `static-auth-secret`, then give the relay
  the same secret and the server's URLs: `TURN_SECRET=<that secret>` and `TURN_URLS=turn:turn.example.com:3478?transport=udp,turns:turn.example.com:5349?transport=tcp`
  (in `/etc/kelpie-relay.env`, or the environment workerd starts with).
- **Cloudflare's TURN** (on Cloudflare): create a TURN key under Realtime → TURN Server, then `wrangler secret put TURN_KEY_ID` and
  `wrangler secret put TURN_API_TOKEN` (the key's API token). Cloudflare bills TURN traffic after its free allowance.

The relay mints a credential (20 minutes) only for a room's own desktop, signed with its room key, and only while the room has a phone allowed
in: at most 10 an hour and 40 a day per room, `TURN_MINTS_PER_NETWORK_DAY` (20 unless set) a day per network, and `TURN_MINTS_PER_DAY` (500
unless set) a day for the whole relay. **Only rooms made with your registration key get TURN** unless you set `TURN_OPEN_ROOMS=1`: these limits
count credentials, not bytes, so on a relay with open registration a stranger could make a room, pair a phone of their own and spend your
TURN traffic. A room made before your relay had TURN counts as made without the key: Kelpie offers *Make a new room* in Settings → Mobile after you
remove a phone (or once the relay has been slow for a day); until the room is remade, its TURN requests get `404 turn_not_offered` and the
phone says *This relay offers no TURN*. Each computer also holds
its screens to its own TURN caps (1 GB a screen, 3 GB a day, 30 GB a month by default; Settings → Mobile → Advanced). The relay cannot read
monthly TURN traffic by itself: set a usage notification in your TURN provider's dashboard, and remove the secrets to stop minting at once.
With no TURN secret set, `/v1/turn` answers `404 turn_not_configured` and the phone shows the TURN way as unavailable (*This relay offers no TURN*), asking again after ten
minutes.

## Serving the phone app from your own Cloudflare account

Kelpie derives the app's address from the relay's: for `wss://relay.example.com` it is `https://relay.example.com`, and it only offers
pairing there when `GET /version.json` on that origin names the Kelpie phone app. So the router is deployed with the app's files as its
static assets, to a Cloudflare account of yours:

1. Build the app for your origin and describe it:
   `cd app && KELPIE_HOSTED_ORIGIN=https://relay.example.com pnpm build:phone:hosted`, then
   `python -I scripts/hosted/bundle_manifest.py app/dist-phone-hosted --tag <tag> --commit <commit>` from the repository root (it writes
   `version.json` and the manifest).
2. Copy `app/dist-phone-hosted` to `relay/public`, change the script names, `workers_dev` and the `ROOM_RATE` namespace in
   `relay/wrangler.toml` and `relay/wrangler.room.toml` to yours, and deploy the room script and then the router with the commands below.
3. Set `TICKET_SECRET` on the router (`wrangler secret put TICKET_SECRET -c wrangler.toml`) and, if you want a closed relay,
   `REGISTRATION_KEY`; leave `OPEN_REGISTRATION` as you want it.

**You are then the phones' trust root.** Whoever controls that Cloudflare account, or can deploy to it, can send a phone code that reads
its sessions and types into them while the app is unlocked, and no Kelpie release describes your app: the fingerprints Kelpie
publishes are for Kelpie's relay. `scripts/hosted/verify_live.py --origin https://relay.example.com --tag <tag> --manifest <your manifest>`
checks your own origin the same way. Kelpie's relay is the choice with a published release and an approval step in front of every deploy.

## Kelpie's own relay on Cloudflare

Kelpie's default relay (`wss://kelpie-link-relay.baopeak.workers.dev`) serves the phone app at the same address and has no registration
key, so every Kelpie install can make a room within the limits above (`wrangler.toml` sets the defaults). **It is deployed only by CI
from a release tag**, after the maintainer approves it in a GitHub environment; nothing deploys it by hand.

- **Release** (`.github/workflows/release.yml`, on a `v*` tag): `hosted-build` builds the phone app twice from clean folders, and the two
  builds must give one bundle hash. `publish` makes a draft release with `hosted-phone-manifest.json` (every file and its hash), the
  bundle as `hosted-phone-<version>.tar.gz`, and the hash in the notes. `hosted-staging` then `hosted-production` deploy the router and
  the app, each in the environment of the same name (the maintainer approves each there); production also waits for `publish` and staging.
  After each deploy the job runs `verify_live.py` and `pnpm smoke` with `PAGE=1`. The maintainer publishes the draft only after production
  is deployed and a `verify_live.py --tag vX --manifest <the draft's manifest>` from outside CI exits 0, so the app is live before the
  desktop release that expects it. A draft whose production deploy or check failed is not published.
- **The room script** (`.github/workflows/relay.yml`): the rooms live in a separate script, and deploying it drops every active room's
  sockets (code 1006; phones reconnect by themselves). So it is deployed only when room code changed (`relay/src/room.ts`,
  `registrar.ts`, `entry-room.ts`, `relay/wrangler.room.toml`). The maintainer starts it from the release tag, staging first, off-peak, before
  that release's router: `gh workflow run relay.yml --ref vX -f tag=vX -f target=staging`, then `-f target=production`, each approved in the
  environments `relay-room-staging` and `relay-room-production`. A branch is refused.
- **Staging** is a second, separate relay at `https://kelpie-link-relay-staging.baopeak.workers.dev` with its own scripts, rooms and
  `TICKET_SECRET`. It serves the same bytes as production; phones are not meant to use it.
- **One-time setup**: the four environments (`hosted-staging`, `hosted-production`, `relay-room-staging`, `relay-room-production`) each with
  the maintainer as required reviewer and deployment limited to `v*` tags; `CLOUDFLARE_API_TOKEN` (Workers Scripts: Edit, with an expiry;
  for Kelpie's relay it is the maintainer's own token, the one below) and `CLOUDFLARE_ACCOUNT_ID` as secrets of those environments only, never of the repository. Each router has its own
  `TICKET_SECRET`: `wrangler secret put TICKET_SECRET -c wrangler.toml` (add `--env staging` for staging).
- **Who deployed what**: `wrangler versions list` (or the dashboard) shows a message on every version: `<tag> <bundle sha256>` for the
  router, `<tag> room <commit>` for the room script. A version without a tag in its message was not deployed by CI.
- **Rollback**: fix forward with a new tag, or run the release again for the previous tag so the same pipeline deploys it. In an emergency,
  `wrangler versions rollback` to a version CI made; whether that also restores the app's files has not been confirmed.
- **Break-glass**: by hand, the maintainer's own Cloudflare token (the same one CI holds) is used only for `wrangler versions rollback` to a CI-made version and for secrets (setting
  `REGISTRATION_KEY` closes registration at once, and removing it opens it again), never for deploying code or app files. Each use is noted. Changing
  `TICKET_SECRET` voids every ticket, so every computer has to make a new room.

### Checking what the relay serves

`python -I scripts/hosted/verify_live.py` (from a clone of this repository, standard library only) fetches every file the release manifest lists
from the live origin and compares its hash and size, checks `/version.json` (the tag, the commit and the bundle hash), the response headers
against the manifest's `_headers` rules, and that the service worker's precache list and the scripts the page loads are all in the manifest.

| Flag | Meaning |
| --- | --- |
| `--env production\|staging` | which deployment (default `production`) |
| `--origin URL` | another origin to check |
| `--tag vX` | the release (default: what the origin's `/version.json` says) |
| `--manifest FILE` | a local `hosted-phone-manifest.json` (default: the tag's GitHub release asset) |
| `--rebuild` | also build the tag in a temporary directory and compare the manifests (a build on another platform may differ) |
| `--scripts` | maintainers: compare the deployed router and room scripts with `wrangler deploy --dry-run` of the tag; reads `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` from the environment (use a read-only token) |
| `--bundle DIR` | check a local bundle against the manifest, with no network (CI does this before it deploys) |

Exit 0: everything matched; 1: a difference; 2: it could not finish. It prints paths and hashes only. **It cannot prove what a given phone
received**: it sees what the origin served to this machine at this moment, so code served only to one visitor, at another time, or already
cached on a phone can differ.

## Limits it applies

64 KiB frames, 8 phones a room, 200 messages a second per socket, a 4 MB burst per room, 2 GB a room a month, and rooms unused for 90 days
removed; 30 refused sockets in 10 minutes from one network to one room before the router stops asking the room. They are constants at the top
of `relay/src/room.ts` and `relay/src/worker.ts`; the room-making limits are the variables above.
