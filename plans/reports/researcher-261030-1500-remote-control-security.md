# Remote-control security design: phone -> home desktop coding agents (Kelpie)

Date: 2026-10-30. Method: 9 web searches (snippets only, few primary docs opened) + analyst judgment. `[U]` = unverified / from background knowledge, confirm before relying. Threat framing: phone-side or relay-side compromise = code execution on the PC.

## 0. Bottom line

1. Noise IKpsk1 + QR psk + desktop approval is the right core. Do not swap to TLS/QUIC/MLS/PAKE.
2. Biggest real risks are NOT the crypto: (a) the phone PWA's code origin (supply chain), (b) the tier model: "write" is effectively "control" for coding agents, (c) approval UX on a phone under prompt injection, (d) `http://` LAN cannot use WebCrypto/SW/PWA install.
3. Exposure ranking (safest first): LAN-only > Tailscale (+Tailnet Lock) > own E2E relay > Cloudflare Tunnel+Access > bare public tunnel > Telegram bot.

## 1. Crypto / protocol choices

| Option | Fit for Kelpie | Browser story | Rust maturity | Verdict |
|---|---|---|---|---|
| Noise IKpsk1 (current) | 2-party, 0-RTT reconnect, no CA, pins desktop key from QR, psk binds QR | Needs ChaChaPoly+BLAKE2s: NOT in WebCrypto -> JS lib (noble) [U]; X25519 DH can use WebCrypto | `snow`: independent audit funded by 1Password, fixes validated by Trail of Bits, 2 informational items open (blog 2026-06-16). README may still say "unaudited" [U date] | Keep |
| TLS 1.3 + pinned key | Browser cannot pin raw/self-signed keys for fetch/WS; only WebTransport `serverCertificateHashes` (<=14d certs, Safari support [U]) | Poor | `rustls` mature, widely deployed, past audit [U] | Worse for phone |
| QUIC (iroh/quinn) | Great native P2P + relays; raw pubkey identity | Browser = relay-only wasm, immature [U] | `quinn` no public audit found [U]; iroh audit: none found (search 2026-10-30) | Overkill, native-only |
| WebRTC DTLS | Browser-native P2P, NAT traversal | Good; fingerprint must be authenticated via the Noise channel; IP leak; TURN cost | n/a | Phase 5+ option for latency only |
| MLS | Group messaging | n/a | n/a | Overkill (2 parties) |

WebCrypto X25519: shipped by default in Safari (WebKit Dec 2024, STP Jan 2025), Chrome 133 (Feb 2025), Firefox 130 (Igalia timeline + MDN BCD; no 2026 re-check; raw-key import/export quirks on Firefox/Safari per noble docs [U]). So in 2026 X25519 is usable on all three, but ChaChaPoly/BLAKE2s still need `@noble/*` (audited by Cure53/Trail of Bits [U]) or a wasm snow build. No audited off-the-shelf JS Noise impl known [U]; hand-rolling the ~200-line state machine on noble + test vectors (cacophony/snow vectors) is the safest route.

Key points on IK (Noise spec sec. 7.7 [U]): msg1 payload is only encrypted to the desktop static key (es,ss) -> weak forward secrecy + replayable. Never put commands, tokens or state-changing data in msg1; treat msg1 as non-authenticating of freshness; make the desktop do no side effects before msg2 round-trips. Noise transport needs ordered nonces; a relay that reorders/drops will desync: use explicit-nonce + window or one ordered stream per session, and rekey periodically.

Key storage on phone:

| Option | Protects against | Doesn't protect against | Notes |
|---|---|---|---|
| IndexedDB non-extractable CryptoKey (X25519 `deriveBits`) | Key exfiltration (XSS, disk copy) | XSS/malicious JS using key as oracle | Usable only in secure context (HTTPS). Static key can be non-extractable; psk/session keys cannot |
| IndexedDB raw bytes | Little | Everything | Avoid |
| Native Keychain/Keystore app | Best (hardware-backed, biometric gate) | Store-review compromise | Needs Capacitor/Tauri mobile wrapper; cost + review |
| WebAuthn passkey | Hardware-bound, user-verified, phishing resistant | Not X25519 | Use as step-up/unlock, not as Noise key |

iOS gotchas [U]: script-writable storage evicted after ~7 days idle in Safari tabs (not for installed home-screen PWAs); Safari-tab storage is separate from the installed PWA's -> pair AFTER install or key is lost. Plan re-pair UX.

Secure-context finding: `http://192.168.x.x` is not a secure context -> no `crypto.subtle`, no service worker, no PWA install, no non-extractable keys (secure contexts spec [U]). Also https page -> ws:// LAN is mixed content; Chrome adds Local Network Access prompts [U]. So "LAN http" = noble-only JS crypto, key in plain IndexedDB, no offline shell. Needs a trusted HTTPS cert (Tailscale cert, or mkcert-style import, or relay-hosted PWA).

## 2. Pairing UX

| Scheme | Used by | Strength | Weakness |
|---|---|---|---|
| QR with high-entropy secret, confirm on primary | Signal/WhatsApp linking | Strong if QR secret stays private | QR is a bearer credential; phished via social engineering (GTIG 2025-02-19: UNC5792, UNC4221, APT44 abuse Signal linked-device QR). Signal later added PIN lock, re-auth |
| SAS compare (6-digit/emoji) from ECDH transcript | Bluetooth numeric comparison (MITM chance 1e-6 per try), Matrix SAS | No secret needed; defeats MITM | Needs commitment before reveal and attempt limits |
| PAKE (CPace > SPAKE2 per CFRG reviews; OPAQUE for server passwords) | IoT pairing | Good for low-entropy PIN only | Redundant when QR carries 256-bit psk. IETF TLS list: with one display + one input, PAKE and SAS equivalent |
| Proximity + same account | Apple Continuity | Zero UX | Needs vendor account/BLE; not available |

Verdict: QR psk + SAS is right; PAKE is overkill (adds nothing over 256-bit psk). Key question: is Kelpie's 6-digit code a SAS derived from the Noise handshake hash (channel-binding) or just a second secret? Must be the former: both sides compute `digits = trunc(HKDF(h, "kelpie-sas"))`, phone displays, user types on desktop, desktop compares. Mismatch -> burn the pairing token. Max 3 tries total.

QR attacks:

| Attack | Applies? | Mitigation |
|---|---|---|
| Shoulder surf / photo / screen-share / malware screenshot of QR | Yes, QR = psk | 120 s TTL, single-use (have), show QR only after Hello, hide behind click, SAS typed on desktop, device name + fingerprint on approval |
| QR swap (attacker shows own QR so user pairs phone to attacker's host) | Low harm: phone controls attacker host; but PWA may leak data | Show host name/fingerprint on phone after scan; no secrets sent before confirm |
| Relay/MITM of handshake | Blocked by IK pinning + psk | have |
| Social-engineered approval (Signal-style) | Yes | Approval dialog: device name, IP/relay, SAS matched, requested tier default = read, big "did you just scan a QR?" |
| Approval fatigue / auto-approve by local malware | Local same-user malware already owns PC | Hello prompt is UI consent, not a cryptographic binding [U] |

## 3. Relay threat model (phase 4)

| Capability of malicious/compromised relay | Impact | Mitigation |
|---|---|---|
| Read content | None (Noise E2E) | have |
| Metadata: IPs, who-talks-to-whom, online times, sizes, timing | Leak; keystroke timing is a classic SSH-style leak (Song et al. 2001 [U]) | Pad frames to fixed buckets, batch input 20-50 ms, avoid per-key frames |
| Drop / delay / reorder | DoS, desync | Window nonces, ack/resync, UI shows "stale" |
| Replay frames | Rejected by nonce | Never reuse a session across restarts |
| Replay msg1 | Desktop wakeups only | No side effects pre-msg2, rate limit per room |
| Room-id enumeration / squatting | Connection flood, billing DoS (Workers/DO are metered) | 128-bit random room id derived from desktop key via HMAC, relay requires signed join challenge by registered pubkey, per-IP and per-room limits, WebSocket hibernation |
| Serve malicious PWA JS | Full compromise (endpoint is the JS) | See next table |
| Downgrade | n/a | No version negotiation; fixed protocol string |

Lessons (secondary/background [U]): Tailscale DERP sees only WireGuard ciphertext, control plane learns node keys/metadata -> trust point mitigated by Tailnet Lock; Signal minimizes metadata (sealed sender) but server sees IPs; Matrix homeservers expose rich metadata; Happy (Claude Code mobile client) markets TweetNaCl E2E with QR pairing and zero-knowledge server (App Store listings only; GitHub not checked) -> same model as Kelpie.

PWA code integrity:

| Approach | Status 2026 | Verdict |
|---|---|---|
| SRI | Only for subresources; HTML itself unprotected | Insufficient alone |
| Service worker caching | Updates re-fetched (~24 h check) from origin -> compromised origin ships new SW | Does not fix |
| Meta Code Verify | Extension, Cloudflare-hosted manifests, desktop browsers [U] | Not for mobile |
| WEBCAT (Freedom of the Press, Sigsum transparency log, ePrint 2025/797) | Alpha (Mar 2026), Ethereum Foundation grant 2026-08-05; extension, not mobile Safari [U] | No standard exists; watch only |
| Native app (App Store/Play) | Review + signing, hardware keystore | Strongest, costly |
| Host PWA on desktop daemon (LAN/Tailscale HTTPS), relay only forwards frames | No third-party code origin | Best PWA option |
| Host PWA on separate static origin, reproducible build, hash published, GitHub Pages/CF Pages | Trust shifts to repo+CI | Acceptable fallback; keep relay Worker and PWA on different accounts/origins |

Honest conclusion: with a PWA fetched from any internet origin, "untrusted relay" is only true if the PWA origin is not the relay operator. Say so in docs.

## 4. Exposure options

| Option | Public listener | Who sees plaintext | Auth in front | Key risks |
|---|---|---|---|---|
| LAN http | No | LAN peers (Noise inside OK) | Noise | No secure context; rogue LAN device only reaches Noise port; DoS; home Wi-Fi guests |
| Tailscale | No | Peers only; Tailscale control plane distributes keys; Tailnet Lock (off by default) makes new node keys need signed approval, so even compromised Tailscale can't add nodes; lost disablement secrets = unrecoverable (docs) | WireGuard + ACL + Noise | Phone must have Tailscale app; vendor trust w/o lock; shared tailnet users |
| Cloudflare Tunnel + Access | Yes (hostname) | Cloudflare terminates TLS | IdP/passkey at edge | CF/IdP compromise = RCE path unless Noise inside; account takeover |
| ngrok-style tunnel | Yes | Vendor | None unless added | Scanned within minutes; URL leak |
| Telegram bot | No (outbound poll) | Telegram (bot chats not E2EE, server holds keys, no forward secrecy in cloud chats) | Telegram account only | Bot token theft, SIM-swap/Telegram account takeover => control; no per-device keys |
| Own relay (phase 4) | Yes, opaque | Nobody (Noise) | Noise + signed join | Metadata, DoS, PWA origin, billing |

## 5. Authorization for dangerous actions

Core insight: tiers must be capability-defined, not names. For coding agents:

| Capability | Real power |
|---|---|
| view/read output | Leaks secrets printed in terminals/diffs/env (prompt-inject exfil sink) |
| send chat message | Agent executes under its own policy: with auto-accept it IS shell access |
| write files | Equivalent to code exec (`.git/hooks`, `package.json` scripts, CI, `.vscode/tasks.json`, agent hooks/MCP config) |
| approve tool call | Executes the exact command |
| raw terminal input | Full shell as user |
| manage devices / settings / agent config | Persistence + privilege escalation |

Controls:

| Control | Recommendation |
|---|---|
| Device mgmt, settings, agent hooks/MCP config, pairing | Local-only; never reachable by any remote role |
| Step-up for control tier | WebAuthn user-verifying passkey (platform authenticator) per session, plus time-boxed elevation (e.g., 15-30 min idle, 8 h max) [timings are my suggestion] |
| Transaction confirmation | Daemon builds the approval payload (full command, cwd, tool, risk label) and signs/binds it by nonce; phone only returns decision. WebAuthn does not display text (only SPC on Chrome [U]) so this does not defeat a malicious PWA; limits blast radius only |
| Rate limits | Pairing attempts, approvals/min, handshakes/min per device and IP; lockout -> desktop notification |
| Audit log | Append-only on desktop: device id, tier, action hash, approved command, time, relay IP. Tamper-evident hash chain; surface in UI |
| Kill switch | Desktop: "revoke all + stop remote listener" one click, tray; phone: "lock now". Revoke is authoritative on desktop so it works while the phone is offline; relay should also drop room on revoke |
| Lost phone | Revoke on desktop; biometric unlock gate in PWA; dormant device expiry (30-90 d [suggestion]) |
| Session lifetime | Control tier short; read tier longer; rekey Noise every N MB/hours |
| NIST SP 800-63-4 (July 2025 per Yubico/Corbado, secondary) | Syncable passkeys now AAL2; phishing-resistant definition = channel/verifier binding. Reauth timers: AAL2 about 1 h inactivity, 24 h overall [U, not opened primary] |
| OWASP MASVS / ASVS 5.0 [U, not opened] | Relevant: biometric-bound keys, session timeouts, re-auth for sensitive ops, no secrets in storage, certificate pinning n/a |

## 6. Prompt-injection angle

| Risk | Evidence | Mitigation |
|---|---|---|
| Agent asks approval of malicious command; small screen + fatigue | TrueFoundry (2026): approval fatigue; Claude Code CVE-2026-24887 (find injection bypassed confirmation, <2.0.72, CVSS 8.8); 50-subcommand deny-rule bypass fixed 2.1.90 (Adversa, via TrueFoundry; secondary) | Do not treat agent's own "safe" parse as a boundary |
| Hidden content in command (bidi/zero-width, long padding, truncated display) | Common technique [U] | Show full command monospaced, escape non-printables, no truncation; scroll-to-end gate for long commands |
| Agent-written description misleads | General | Show daemon-computed risk label (network, delete, write outside worktree, git push, env/secret access, install), never agent text, as primary |
| "Always allow" from phone | Persistent weakening | Phone can only approve once; no rule creation remotely |
| Rubber-stamp bursts | Fatigue | Min-delay on approve button for high risk, collapse batches, cap approvals/min |
| Deny-by-default list for remote | Industry practice (sandbox, plan mode, per-tool policies) | Remote cannot approve: sudo, curl/wget piping to shell, package installs, ssh, writes outside worktree, edits to hooks/CI; must be done at desktop |
| Secret leakage in view tier | Exfil via output | Redact known secret patterns in mobile stream [U effectiveness] |

## 7. Ranking of exposure options (security, best first)

1. LAN-only direct (smallest surface; limited by HTTPS/secure-context UX)
2. Tailscale with Tailnet Lock + ACLs, daemon bound to tailnet only, Noise inside
3. Own E2E relay (Noise + signed join, PWA hosted from a separate origin or the desktop)
4. Cloudflare Tunnel + Access (strong edge auth, but CF sees plaintext; combine with Noise to equal #3)
5. Bare public tunnel (ngrok etc.)
6. Telegram bot (no E2E, single account/token trust)

## 8. Must-have controls vs Kelpie plan

| Plan item | Verdict | Note |
|---|---|---|
| Noise_IKpsk1_25519_ChaChaPoly_BLAKE2s | Right | Add: no payload in msg1, ordered/windowed nonces, rekey, fixed protocol string |
| QR psk, single-use, 120 s | Right | Burn on any failed attempt |
| 6-digit code typed on desktop | Right if channel-bound SAS; missing otherwise | Derive from handshake hash h, max 3 tries |
| Windows Hello approval | Right (cheap) | Show device name, fingerprint, requested tier (default read) |
| Trusted-app-process check | Marginal/overkill | Same-user malware can spoof or inject; keep as hygiene only |
| Per-device read<write<control | Right idea, wrong granularity | Redefine by capability (section 5); write==control |
| Device records MAC'd with keyring key | Low value | Same-user malware reads keyring; defends only disk tamper. Fine but do not rely |
| Missing in phase 3 | Must add | Remote-forbidden ops list; audit log; kill switch; rate limits; command-display with daemon risk labels; deny-by-default for remote approve; session lifetime; HTTPS secure-context solution; iOS storage/re-pair UX; dormant device expiry |
| Phase 4 relay | Right design | Add: signed join, per-room/IP limits, padding/batching, billing guard, separate PWA origin, document that relay-hosted PWA is a trust point |
| PAKE / MLS / iroh / WEBCAT / Tailnet Lock integration | Overkill now | Document Tailscale as optional hardened path |

Priority order: (1) capability tiers + remote-forbidden ops, (2) channel-bound SAS + attempt burn, (3) full-command approval UX with daemon risk labels, (4) audit + kill switch, (5) HTTPS/secure-context decision, (6) relay limits and PWA origin separation, (7) step-up passkey for control.

## Limitations

Mostly search snippets; primary specs (Noise, NIST 800-63B-4, ASVS 5, MASVS) not opened. Dates for iOS/ITP, Chrome LNA, Code Verify status are from background knowledge [U]. Did not inspect Kelpie's actual phase 3 code, only the described plan. Happy repo not read.

## Sources (accessed 2026-10-30)

- Igalia, Secure Curves status: https://blogs.igalia.com/jfernandez/?p=1913 ; Mozilla bug https://bugzil.la/1904836
- 1Password, snow audit (2026-06-16): https://1password.com/blog/strengthening-snow-for-the-open-source-community
- Tailnet Lock: https://tailscale.com/docs/features/tailnet-lock
- WEBCAT: https://freedom.press/tech/news/introducing-webcat-web-based-code-assurance-and-transparency/ ; https://eprint.iacr.org/2025/797 ; https://blog.ethereum.org/2026/08/05/1ts-grant
- GTIG Signal QR phishing (2025-02-19): https://cloud.google.com/blog/topics/threat-intelligence/russia-targeting-signal-messenger
- PAKE vs SAS pairing (IETF TLS list): https://mailarchive.ietf.org/arch/msg/tls/cxQGtCc8iA8jIiPUf-SXEQ-9nWo/ ; CFRG PAKE review https://mailarchive.ietf.org/arch/msg/cfrg/eo8O6JYPmWY6L9TlcIXStFy5gNQ/
- Claude Code injection/approval bypasses (secondary): https://www.truefoundry.com/blog/claude-code-prompt-injection ; https://cvefeed.io/vuln/detail/CVE-2026-24887
- NIST 63-4 passkeys (secondary): https://yubico.com/blog/new-nist-guidance-on-passkeys-key-takeaways-for-enterprises
- Telegram bot non-E2E (secondary): https://reclaimthenet.org/is-telegram-encrypted
- Happy listing: https://apps.apple.com/app/id6748571505

## Unresolved questions

1. Is the 6-digit code bound to the handshake transcript, or an independent secret?
2. Where is the phase-3 phone UI served from, and over what HTTPS cert? (secure-context blocker)
3. Is "write" intended to include chat messages that trigger agent execution and workspace file writes? Needs capability redefinition.
4. Who hosts the PWA in phase 4: same Cloudflare account as relay?
5. Native wrapper (Capacitor) acceptable for hardware-backed keys, or PWA-only?
6. Can the daemon enforce its own risk labels independent of each agent adapter (Claude/Codex/ACP)?
7. Primary-source verification of NIST 800-63B-4 session timers and ASVS 5 requirements.
