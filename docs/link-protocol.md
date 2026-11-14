# kelpie-link and the relay: the wire

How a phone reaches Kelpie on a computer, byte by byte. The end-to-end protocol (`kelpie-link`) is the same over every carrier; the relay
is one more carrier, which forwards what it cannot read. The reasons behind each choice are in [architecture.md](architecture.md),
"Phone link"; this page is what an implementation needs.

## kelpie-link

Implemented three times and checked against the same vectors: `crates/kelpie-link` (the daemon), `app/phone/src/noise.ts` (the phone) and
`scripts/e2e/link_noise.py` (the tests). Vectors: [`crates/kelpie-link/tests/vectors.json`](../crates/kelpie-link/tests/vectors.json), plus
the published cacophony ones in `cacophony-ik.json`.

- **Patterns**: `Noise_IKpsk1_25519_ChaChaPoly_BLAKE2s` to pair (the 32-byte secret from the QR code at position 1), `Noise_IK_25519_ChaChaPoly_BLAKE2s`
  to reconnect. Prologue `kelpie-link/1` followed by the mode byte.
- **First message**: `[version = 1, mode (1 pair, 2 reconnect), mac1 (16 bytes)] + Noise message 1`. `mac1` is keyed BLAKE2s-128 over
  `[version, mode] + message`, the key being `BLAKE2s("kelpie-link mac1 desktop" || desktop public key)` to reconnect or
  `BLAKE2s("kelpie-link mac1 psk" || secret)` to pair. A first message whose `mac1` fails is dropped unanswered, before any Diffie-Hellman.
- **Reply**: the bare Noise message 2, with no payload.
- **Then**: each carrier message is one Noise transport message (at most 65,535 bytes) holding `[flags] + bytes`; flag `1` means more chunks
  follow. One application message is at most 1 MiB. Each direction rekeys every 2^16 messages by counting. A message that does not decrypt ends the
  connection: there is no reorder window and no resumption, and every reconnect is a new handshake.
- **First sealed frame**: `{type: "pair", name, want}` when pairing, `{caps, resume?, relay_token?}` when reconnecting. `caps` holds `ack`
  and `hb` (a client without both is served as one that acknowledges nothing, and is refused on the relay), `relay` from a client that can use a relay token, and `pad` from one that reads padded chunks.
- **Padding**: a chunk's plaintext may be padded so that the carrier sees only a few sizes: flags with bit `2` set, the bytes, zeros, and
  the number of zeros as a u16 big-endian last, the whole rounded up to the smallest of 256, 1,024, 4,096, 16,384 or 65,519 bytes that holds it
  (a padded chunk carries at most 65,516 bytes). Filler that is not zeros, or a count larger than what is there, is a broken connection.
  Every end reads padded and unpadded chunks; an end pads only once the other has said it reads them: the computer once the phone's `caps`
  has `pad`, from its hello on, and the phone once that hello says `pad: true`. Pairing messages are not padded. Nothing is ever sent only to
  fill the line: the timing and number of frames still show. Vectors: [`crates/kelpie-link/tests/padding.json`](../crates/kelpie-link/tests/padding.json).
- **Approval code**: `HKDF-BLAKE2s(ikm = handshake hash, no salt, info = "kelpie-sas")`, its first 4 bytes big-endian, modulo 10^6.
- **QR payload** (`#p=`): base64url of `{v: 1, dpk, psk, ep: [{t, a, r?}], name}`. Endpoint kinds: `ts` (`a` = `<name>:<port>` of the HTTPS
  origin), `relay` (`a` = `wss://<host>`, `r` = the room's ticket), `lan` (`a` = `ip:port`). A client skips kinds it does not know.
  A code for the hosted phone app has the relay as its only endpoint, and its link is on the relay's own origin (`https://<host>/#p=…`).

The JSON messages inside the session are the phone protocol, described in architecture.md, "Phone protocol (v2)". The relay adds two:

- `{type: "relay", url, ticket, token, expires}` from the computer, after the hello (and in `paired` as `relay`): this phone's token for the
  relay, base64url of 32 random bytes, good until `expires` (ms). The computer keeps only its SHA-256 and replaces it after 7 days.
- `addrs` carries the relay as an endpoint while it is on, and leaves it out when it is turned off. A page the relay hosts and a locked phone hear `addrs {partial: true}`: the relay alone, nothing of the computer's other addresses or its hostname (the hosted page still has the computer's name from its QR code, and the room is told the Tailscale origin among the pages it lets in).

### Screen over WebRTC

From API 42 the computer's screen may go peer to peer (Windows). Implemented in `crates/kelpie-rtc` with `crates/kelpied/src/screen/rtc.rs` (the
computer, the answerer) and `app/phone/src/screen-rtc.ts` (the phone, the offerer).

- **Signalling, inside the session only**: the phone sends `{type: "sc_rtc_offer", sc, sdp}` (at most 16 KiB) after ICE gathering or 2 s, and
  the computer answers `{sdp, nonce, ice_servers}`; later phone candidates go as `{type: "sc_rtc_ice", sc, cand}` (an SDP `candidate:` line).
  The computer's candidates are all in its answer. `{type: "sc_via", sc, via}` (`auto`, `p2p`, or the link's own `relay` / `tailscale`) sets the
  way from then on; the computer says where the picture goes with `{type: "sc_path", sc, path, state, why?}` (`state` `live` or `failed`).
- **Binding**: the offer and the answer travel only inside the end-to-end encrypted session, so the DTLS fingerprints they carry are those of
  the two ends, and a DTLS peer with another certificate is refused. The first message on channel `"i"` must be
  `{"type": "rtc_hello", "sc": <sc>, "nonce": <the answer's nonce>}`; anything else closes the peer before any media goes on it.
- **Channels**, negotiated by id (no in-band open): 0 `"m"` reliable and ordered, 1 `"c"` unordered with no retransmission, 2 `"i"` reliable
  and ordered.
- **On `"m"`**: each media message (the link's binary format, `0x01` first) in chunks of at most 16,384 bytes,
  `[0x02, msg_seq u32 LE, idx u16 LE, count u16 LE] + piece`. A message whose chunks were cut short by a newer one is dropped whole.
- **On `"c"`**: cursor messages whole (`0x01` first, kind 3), the latest only.
- **On `"i"`** after the hello: `{"type": "sc_ack", "seq"}` and `{"type": "sc_input", "ops", "st"}` as on the link, without `sc` (the channel
  is the screen's). They are taken only while the screen's link is connected.
- **TURN** (API 43): before its offer the phone asks `{type: "sc_rtc_servers", sc, relay_only?}`, answered `{ice_servers, turn}` (`turn` null, or
  why there is none: `turn_not_configured`, `turn_full`, `turn_budget_spent`, `method_off`, `relay_off`, `turn_failed`, `no_turn_direct`). The
  computer mints for a screen through the relay only, and five minutes before they end, while the pair goes through TURN, sends new ones as
  `{type: "sc_rtc_servers", sc, ice_servers}`, with which the phone opens a new peer (swapped once open). An offer through TURN alone carries
  `relay_only: true` (the phone's `iceTransportPolicy: "relay"`). `sc_stats` carries `turn_mb` once anything went through TURN; past an
  allowance the picture goes back to the link (`sc_path {why: "turn_budget_spent"}`), or the screen ends with `turn_budget_spent`.
- **Each side sees the other's IP addresses** (its host and public candidates); the relay sees only the encrypted session.

## The relay

Source: [`relay/`](../relay). A Cloudflare Worker (the router, `src/worker.ts`) and one Durable Object per room (`src/room.ts`), with one more
Durable Object that counts the rooms made in a day (`src/registrar.ts`), or all in one workerd process when self-hosted
([relay-self-host.md](relay-self-host.md)). On Cloudflare the router also has the phone app's files beside it as static assets.

### HTTP

| Request | Answer |
| --- | --- |
| `GET /v1/health` | `200 ok` |
| `POST /v1/rooms`, body `{"pk": "<P-256 public key, uncompressed, base64url>"}` (1,024 bytes at most), no `Origin`; `Authorization: Bearer <registration key>` when the relay has one | `201 {"room", "ticket"}`; see "Making a room" for the refusals |
| `GET /v1/desktop?t=<ticket>&ts=<ms>&sig=<signature>` (WebSocket) | 101 with `x-kelpie-relay-version: 2`, or 403 a bad ticket, signature or `Origin`, 404 no such room, 429 the month's cap is spent |
| `GET /v1/phone?t=<ticket>&k=<token>` or `&p=<pairing token>` (WebSocket) | 101, or 403 a bad ticket, an unknown token or an `Origin` the room does not list, 409 eight phones already, 429 the cap |
| `POST /v1/turn?t=<ticket>`, body `{"ts", "sig"}` (1,024 bytes at most), no `Origin` | `200 {"ice_servers", "username", "expires"}`: TURN credentials for 20 minutes; see "TURN" for the refusals |
| `POST /v1/turn/revoke?t=<ticket>`, body `{"ts", "sig", "username"}` | `204`: the credential ends now (coturn's cannot: `204`, and it expires on its own) |
| any other path under `/v1` | 404, 405 or 426, in plain text |
| any path outside `/v1` | on Cloudflare: the phone app's static files, or the app's own 404 page, answered without running the Worker; on workerd: 404 in plain text, since a self-hosted relay serves no page |

The Worker's own answers repeat nothing of the request, because the page runs on the same origin: plain text or JSON, `nosniff`, no cache.

#### Making a room

Who may is the relay's setting, and it is checked in this order:

1. A `REGISTRATION_KEY` is set: only its holder, `Authorization: Bearer <key>` (at most 256 characters). A missing or wrong key is `401`; 10 wrong
   keys from one network in a day are answered `429` after that (no `x-kelpie-reason`).
2. No key, and `OPEN_REGISTRATION` is `1`: anyone. The router first sheds a burst per network (Cloudflare only, 5 a minute): `429` with
   `x-kelpie-reason: rate`. The registrar then counts rooms per UTC day and per network (`ROOMS_PER_NETWORK_DAY`, 20 unless set): `429` with
   `x-kelpie-reason: network`; and for the whole relay (`ROOMS_PER_DAY`, 1,000 unless set): `503` with `x-kelpie-reason: full`.
3. Neither: `503`, "relay not configured".

Any `Origin` is `403` (a page can never make a room), a body announced as over 1,024 bytes `413`, a key that is not a P-256 public key `400`. A network is
the hash of `Cf-Connecting-Ip` (an IPv6 address by its /64, an IPv4 address whole); no address is stored.

A ticket is no secret (every QR code has one), so the router counts the sockets a room refused (403, 404) by a hash of the network (an IPv6
address by its /64, an IPv4 address whole), the path and the room: phones' refusals never count against the desktop. Past 30 in 10 minutes it
answers 429 with `x-kelpie-reason: attempts`, without asking the room; a 429 without it is the room's monthly cap. A full table lets go of
expired counts, then of the oldest, never of all at once. In the isolate's memory only, so best effort.

- **Ticket**: `<room id, 32 hex>.<HMAC-SHA256(TICKET_SECRET, room id), base64url>`. Checked in the router before any Durable Object call.
- **Desktop signature**: ECDSA P-256 / SHA-256 with the room key, `r || s` base64url (86 characters), over `kelpie-relay/1 desktop <room> <ts>`.
  `ts` must be within 5 minutes of the relay's clock and later than the last one the room took.
- **Tokens**: the room keeps SHA-256 (hex) of each 32-byte token. A pairing token is `HKDF-SHA256(ikm = the QR code's secret, no salt,
  info = "kelpie-relay-pair", 32 bytes)`, let in once and for at most 120 s.
- **Origin**: the desktop's socket must have none. A phone's, when it has one, must be one of the (up to four) origins the desktop registered
  (`origins`), which are the pages its phones come from: the computer's Tailscale origin and, once the relay serves the phone app, the
  relay's own. It guards against other pages only; the token is what lets a phone in.
- **Version**: the desktop's 101 carries `x-kelpie-relay-version: 2`. A desktop that reads none (an older relay) speaks the first version:
  it sends `origin`, never `origins`, and offers no hosted app. A relay of the first version closes a socket on a message it does not know.

#### TURN

For a screen through the relay that cannot go straight between the phone and the computer (`relay/src/turn.ts`, `crates/kelpied/src/mobile/turn.rs`).

- **Signature**: as the desktop's, over `kelpie-relay/1 turn <room> <ts>` to mint and `kelpie-relay/1 turn_revoke <room> <ts> <username>` to
  revoke; `ts` within 5 minutes and later than the last one the room took for TURN (its sockets keep their own). A bad ticket, signature or
  time is `403` and counts against the network as a refused socket does.
- **Who**: a room made with the registration key, or any room where `TURN_OPEN_ROOMS` is `1` (else `404` with `x-kelpie-reason:
  turn_not_offered`: the limits count credentials, not bytes, and anyone can make a room on an open relay); the room must have at least one
  phone on its allow-list (`409`, `no_phones`); at most 10 mints an hour and 40 a day per room (`429`, `turn_room_limit`),
  `TURN_MINTS_PER_NETWORK_DAY` (20 unless set) per network (`429`, `turn_network`) and `TURN_MINTS_PER_DAY` (500 unless set) for the whole
  relay (`503`, `turn_full`) per UTC day. Rooms keep whether they were made with the key (`keyed`); a room from before counts as open.
- **What**: with secrets `TURN_KEY_ID` and `TURN_API_TOKEN`, Cloudflare's TURN (`generate-ice-servers`, TTL 1,200 s); with `TURN_SECRET` and
  `TURN_URLS` (comma-separated), coturn's time-limited credentials: `username = <expiry, s>:<room>`, `credential = base64(HMAC-SHA1(secret,
  username))`. With neither, `404` with `x-kelpie-reason: turn_not_configured` (a relay from before TURN answers `404` too). URLs on port 53
  are left out; an upstream failure is `502` `turn_upstream`. The API token never leaves the Worker, and nothing about a credential is logged.
- **Vectors**: the statements and a coturn credential are in [`relay/test/vectors.json`](../relay/test/vectors.json).

### On the sockets

Binary frames only carry kelpie-link, at most 64 KiB. A phone's frame reaches the desktop as `[slot, u16 big-endian] + frame`; the desktop's
frame names the slot it goes to. Nothing is queued: with no desktop ready, a phone's frame is dropped, and counts against neither the burst
nor the month. Only the newest desktop socket is heard: what a replaced one still sends, text or frames, is ignored. A slot is a number for one phone
socket, taken from a counter the room keeps and never given out while any socket, open or closing, still holds it: a late `close`, frame or
`kick` of an old socket cannot reach the phone that connected after it. The daemon also numbers each `open` of a slot, and drops what an older
session of it still sends.

Text `ping` is answered `pong` by the runtime, without waking the room. Anything else in text from a phone closes it (4003). The desktop's
text is JSON:

| From the desktop | Meaning |
| --- | --- |
| `{"t": "allow", "h": ["<sha256 hex>", …]}` | The whole allow-list (64 at most). Phones whose token left it are closed (4002). The first one on a connection makes the desktop ready: only then does it hear of phones, and phones of it. |
| `{"t": "pair", "h": "<sha256 hex>", "ttl": <1–120 s>}` | A pairing token, once (4 live at most). |
| `{"t": "origins", "o": ["https://<host>", …]}` | The page origins phones may come from (4 at most), replacing the list. |
| `{"t": "origin", "o": "https://<host>"}` | The one page origin, from a desktop that predates `origins`; the relay keeps it as a list of one. |
| `{"t": "kick", "s": <slot>, "c": <4000–4999>, "r": "<reason>"}` | Close that phone's socket with that code. |
| `{"t": "drop"}` | Remove the room: every socket closes (4004) and nothing of it is kept. |
| `info` | Counts only: the object's id, start, uptime, sockets, version. |

| From the relay | To | Meaning |
| --- | --- | --- |
| `{"t": "open", "s", "via": "token" \| "pair", "k": "<sha256 hex>", "o": "<origin>"}` | desktop | A phone is on slot `s`, let in by the token (or pairing token) with hash `k`, from the page `o` (left out for an app that sends no `Origin`). The daemon's limits go by `k`, since a slot is new for every socket. The daemon pairs a phone only when `o` is the page its pairing code was issued for, and a device later only from the page it paired on; the relay could only lie about `o` with no key to use it. |
| `{"t": "close", "s", "c"}` | desktop | That phone left, closing with code `c` (1006 when its socket just dropped). 4000 is a phone's route that lost its race, which the daemon does not count as a failed attempt. |
| `{"t": "desktop_offline"}`, `{"t": "desktop_online"}` | phones | The desktop left, or is back. Hints only: a relay can say anything, so they only ever end a connection. |

### Close codes

| Code | Why |
| --- | --- |
| 4001 | Replaced: a newer desktop, or a newer socket with the same token. |
| 4002 | The token is no longer on the allow-list. |
| 4003 | Text that is not accepted. |
| 4004 | The room was removed. |
| 4008 | More than 200 messages in a second from one phone, or sent to one phone by the desktop. A desktop over the limit closes that phone, never itself. |
| 4009 | The room went past its burst of 4 MB (refilled at 4 MB a second); for the desktop's frames, the phone they were for is closed. |
| 4010 | The room's 2 GB for the month (UTC) are spent. |
| 1009 | A frame over 64 KiB. |
| 1006 | No close frame at all, which is what a code update on Cloudflare gives. Clients take it as any other drop. |

From the phone, on its own route: 4000 closes the loser of its race (another route finished its handshake first), which the computer does not count as a failed attempt.

From the computer, through `kick`: the codes of `/link` (architecture.md), and 4412 for a device that may not use the relay (paired over plain HTTP, or without
heartbeats).

### Limits and storage

- A room stores its id, its public key, the last signature time, the allow-list, live pairing tokens, the phones' origins (`origins`; a room
  made before there could be several keeps its one `origin`, until the desktop sends a list), the next slot number and its byte count for the
  month, written as messages arrive, in batches (once a minute, or after each MB), never on a timer. The registrar stores, for each UTC day,
  the number of rooms made in all and per network (a hash, never an address), and drops the days before yesterday. An unused room is removed 90 days
  after its last connection, on one alarm set each time it is used; a desktop still connected when the alarm comes counts as use.
- **How the daemon ends its connection**: when the relay is turned off or moves, at once. When phone access goes off, the kill switch or a reset:
  for a second it passes on what the sessions still send (a removed phone's sealed `revoked`), then sends `allow` with nothing, so nobody is let
  in; a reset sends `drop` instead, and its phones see 4004. A new room drops the old one the same way, after phones were told the new ticket. A device change reaches
  the relay a second after it is made, so a removed phone hears `revoked` in its session and is closed by the daemon (4401) before the relay
  would close it for its token (4002).
- No payload is ever logged or stored; Workers observability is off. Cloudflare's own edge keeps its logs (connection metadata), as any
  network on the way does.
- Room placement: no location hint. The room is made by the desktop's own request, so it lands near the desktop.

### Vectors

[`relay/test/vectors.json`](../relay/test/vectors.json): a room key, a desktop signature made with WebCrypto, a pairing secret with its token,
and a device token with its hash. `relay/test/relay.test.ts` and `crates/kelpied/src/mobile/relay.rs` both check them, and the phone's tests
check the pairing token and the token hash.
