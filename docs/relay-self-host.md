# Running the relay yourself

Kelpie's phones can reach a computer through a relay when Tailscale is off on the phone (Settings → Mobile, *Reach this computer through
the relay*). By default that is the relay Kelpie's maintainer runs on Cloudflare. The same code runs on
[workerd](https://github.com/cloudflare/workerd), Cloudflare's open-source runtime, so you can run your own: it sees what any relay sees
(that a phone and a computer are connected, their addresses, when and how much), and never what they say. The wire is in
[link-protocol.md](link-protocol.md).

## What you need

- A machine reachable from the internet, with a name and a TLS certificate: phones and Kelpie connect with `wss://`, and the phone app's
  page is on HTTPS, so it cannot use a plain `ws://` relay.
- Docker, or Node 24 with pnpm 11 to build it yourself.
- Two secrets, which never go in a file of the repository:
  - `TICKET_SECRET`: signs the rooms' tickets. 32 random bytes, for instance `openssl rand -base64 32`.
  - `REGISTRATION_KEY`: what Kelpie needs to make a room. Anyone who has it can make rooms on your relay, so give it only to the
    computers you mean to serve.

## With Docker (one command)

```bash
cd relay
docker build -t kelpie-relay .
docker run -d --name kelpie-relay --restart unless-stopped -p 127.0.0.1:8787:8787 -v kelpie-relay:/data \
  -e TICKET_SECRET="$(openssl rand -base64 32)" -e REGISTRATION_KEY="<a long random key>" kelpie-relay
```

The rooms live in the `kelpie-relay` volume. `TICKET_SECRET` must stay the same across restarts: tickets made with another one are refused,
and every computer would have to make a new room. Keep it where your secrets live (a Docker secret, an environment file only root can read).

## Without Docker

```bash
cd relay
pnpm install --frozen-lockfile
pnpm build:selfhost              # writes workerd/dist/entry.js
cd workerd
TICKET_SECRET=... REGISTRATION_KEY=... ../node_modules/.pnpm/@cloudflare+workerd-*/node_modules/@cloudflare/workerd-*/bin/workerd serve config.capnp
```

It listens on port 8787 and keeps rooms in `workerd/rooms`.

## TLS in front

Put a reverse proxy with a certificate in front, for instance [Caddy](https://caddyserver.com):

```
relay.example.com {
  reverse_proxy 127.0.0.1:8787 {
    header_up Cf-Connecting-Ip {remote_host}
  }
}
```

The relay counts wrong registration keys by a hash of `Cf-Connecting-Ip`, which Cloudflare sets for it; behind your own proxy, set it as
above, or every caller shares one count. Only that path uses an address: what phones send is limited per token and per room.

## Checking it

```bash
cd relay
RELAY_URL=https://relay.example.com REGISTRATION_KEY=... pnpm smoke
```

It makes a room, signs in as a desktop, lets a phone in with a token, sends 100 frames each way, revokes the token and removes the room.

## Using it

In Kelpie, Settings → Mobile, *Reach this computer through the relay*: put `wss://relay.example.com` as the relay's address, paste the
registration key, and turn it on. The key is used once, to make this computer's room, and kept in the computer's credential store for making
a new room later. Phones paired over Tailscale get their token the next time they connect.

## Kelpie's own relay on Cloudflare

Kelpie's default relay (`wss://kelpie-link-relay.baopeak.workers.dev`) is deployed by hand with wrangler, never from CI; `.github/workflows/relay.yml`
only runs the checks (typecheck, tests, and the self-hosted bundle smoke-tested on a local workerd). With `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID` set in that shell only:

```bash
cd relay
pnpm install --frozen-lockfile && pnpm typecheck && pnpm test
pnpm deploy:room       # the Durable Object script first: the router binds to it
pnpm deploy:router
# once, or to replace one (never in a file of the repository):
wrangler secret put TICKET_SECRET -c wrangler.toml
wrangler secret put REGISTRATION_KEY -c wrangler.toml
RELAY_URL=https://kelpie-link-relay.baopeak.workers.dev REGISTRATION_KEY=... pnpm smoke
```

A router deploy leaves live rooms alone; a room-script deploy drops active rooms' sockets (code 1006), and phones reconnect by themselves.
Deploy rarely and off-peak, and roll back with `wrangler rollback`. Changing `TICKET_SECRET` voids every ticket, so every computer has to make a
new room.

## Limits it applies

64 KiB frames, 8 phones a room, 200 messages a second per socket, a 4 MB burst per room, 2 GB a room a month, and rooms unused for 90 days
removed. They are constants at the top of `relay/src/room.ts`.
