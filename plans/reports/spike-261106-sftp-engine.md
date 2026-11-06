# SFTP engine spike (decides F1)

Date 2026-11-06. Tool: `scratch/sftp-spike/` on the throwaway branch `spike/sftp-engine`, never merged.

**Basis: Windows 10 (19045) and the paramiko (Python) fixture only.** No Windows 11, no macOS and no real OpenSSH server were available. Every throughput figure is measured against a Python SFTP server; ratios between engines in the same run are the meaningful numbers, not the absolute MB/s. The **agent-only key** case was **skipped**: the Windows ssh-agent service is stopped, and the owner chose not to start it.

## Setup
- **Client and server**: `OpenSSH_for_Windows_9.5p1, LibreSSL 3.8.2`; `scp -3` is offered. The server is `scripts/e2e/ssh_test_server.py` (paramiko 5.0.0) on 127.0.0.1.
- **Latency**: 1 ms and 50 ms round trip through `scripts/e2e/net_fault_proxy.py`.
- **Client launch** (what the daemon will use): `ssh -F <generated-format config> -o NumberOfPasswordPrompts=1 -o ConnectTimeout=15 -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -o LogLevel=ERROR -s <dest> sftp`, with `kill_on_drop`, `CREATE_NO_WINDOW` and the askpass environment.
- **Engines**:
  - A1: russh-sftp 3.0.1;
  - A2: openssh-sftp-client 0.15.12, `Sftp::new` on the child's stdio, without the `openssh` feature;
  - baseline: the sftp CLI (64 × 32 KB requests, its connect time subtracted).

## Results
### Requirement 1 (1 GB, SHA-256 on both ends) and 2 (throughput, MB/s)
| Path | A1 up | A1 down | A1 (16 req) up / down | A2 up | A2 down | A2 down, 16 parallel handles | CLI up | CLI down |
|---|---|---|---|---|---|---|---|---|
| direct, 1 GB | 12.2 | **33.1** | 8.7 / 30.8 | 8.6 | 8.0 | 14.1 | 23.3 | 28.3 |
| 1 ms RTT, 256 MB | 3.1 | 9.6 | 3.1 / 15.3 | 3.6 | 0.6 | 24.5 | 24.9 | 59.1 |
| 50 ms RTT, 256 MB | 8.9 | **31.7** | 7.6 / 28.3 | 9.0 | ≈0.33 (stopped after 137 s) | 4.6 | 31.3 | 30.4 |

Every completed transfer had equal SHA-256 hashes. The 1 ms proxy row is noisy: the proxy's 16 KiB asyncio chunks cost more than the latency itself.

### Upload diagnosis (A1 at 255 KiB write packets; A2 and the CLI in the same run, 256 MB)
| Variant | direct up | 50 ms up |
|---|---|---|
| A1 as above | 12.3–12.7 | 10.8–11.2 |
| A1, 255 KiB write packets | 13.5 | 10.2 |
| A1, 1 MiB anonymous stdin pipe | 13.6 | 10.9 |
| A1, overlapped named pipe for stdin (as `sftp.exe` gives `ssh.exe`) | 13.2 | 10.5 |
| A1, a write buffer before the pipe | deadlock: russh-sftp never flushes its stream | |
| A2, overlapped named pipe | 11.8 | 9.9 |
| CLI | 64.5–69.7 | 27.2–27.3 |

Uploads from both crates stay at 9–13 MB/s, whatever the write size, the pipe buffer, the pipe type or the RTT. The CLI reaches 23–70 MB/s against the same server, and A1's **downloads** match or beat the CLI. So the limit is neither request pipelining nor the stdin pipe. The cause is **not proven**. The next candidates are:
- how the Python server handles the crates' write pattern, which only a real OpenSSH server can rule out;
- the per-response read path of a tokio `ChildStdout` on Windows.

### Requirement 3 (resume)
A1 and A2 both pass, for upload and download. A file reopened without truncate and seeked to 89 490 830 bytes (not on a chunk boundary) ends with equal SHA-256.

### Requirement 4 (auth)
- **Key, saved password through askpass, ProxyJump**: A1 and A2 connect in 0.17–0.38 s.
- **Unknown host key**: fails in 0.09–0.13 s, and ssh's own text "Host key verification failed." is kept.
- **Agent-only key**: skipped (see above).

### Requirement 5 (extensions, paramiko)
- Version 3, extensions `{check-file: md5,sha1}` only: no `posix-rename`, `statvfs` or `fsync`.
- A plain rename over an existing file fails (`Failure`).
- The `posix-rename@openssh.com` request is refused, and the old target stays.

So the move-aside replace in `files/copy.rs` is the path this server needs. A real OpenSSH server offers `posix-rename` (not measured here).

### Requirement 6 (names)
- Unicode, emoji, CJK, spaces, a leading dash, `;` and quotes, and a 200-character name all round-trip with A1.
- Not testable on NTFS: non-UTF-8 bytes, `\` and newline. From the source, russh-sftp decodes non-UTF-8 names lossily (U+FFFD), so they cannot round-trip. `files::readable_name` flags them, as the plan intends.

### Requirement 7 (versions)
`OpenSSH_for_Windows_9.5p1, LibreSSL 3.8.2` on Windows 10 19045. `scp -3` exists.

### Requirement 8 (kill mid-transfer, key host and ProxyJump host)
- **Detection**:
  - A1 notices a killed or dropped child within 2–38 ms ("session closed" / "sender dropped");
  - A2 within 24–271 ms ("read/flush task failed").
- **Orphans after kill or drop**: none, including the ProxyJump inner `ssh.exe`.
- **Hard kill of the owning process** (a daemon crash): `ssh.exe` and the inner ProxyJump `ssh.exe` exit by themselves when their pipes close (no orphans, four cases). The phase-2 job object is still worth having as a backstop, but nothing showed it was needed.
- Caveat: two `kill` rows recorded an empty process tree, because the process listing was slower than the kill. The ProxyJump rows did record the full tree, with no orphans.

### Requirement 9 (binary size, stripped release, thin LTO)
| Build | Size |
|---|---|
| spike with no engine | 448 KiB |
| A1 | +753 KiB |
| A2 | +856 KiB |

kelpied already links tokio, serde and others, so the real increase will be smaller.

## Gate (as the plan writes it)
| Requirement | A1 | A2 |
|---|---|---|
| 1 transfer and SHA | pass | pass |
| 3 resume | pass | pass |
| 4 auth (agent case skipped) | pass | pass |
| 8 kill noticed, no orphans | pass | pass |
| ≥ 70 % of the CLI at 50 ms RTT | **upload 0.28, download 1.04: fail** | upload 0.29, download ≈0.01: fail |

**Literal verdict: B** (`sftp -b`, no live progress). The owner allowed B only after both A1 and A2 fail, which they do here.

**Not decided here** (an owner decision):
- The failure is upload-only.
- It is shared by both crates.
- It does not move with any engine-side setting, and it is measured against a Python server.
- A1 passes everything else and downloads as fast as the CLI.

B would give up per-file live progress, the streamed walk and the in-process resume checks. Options are listed in the progress report: `plans/reports/fullstack-developer-261106-0130-file-transfer.md` (main worktree).
