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

## Upload diagnosis plan (the owner chose option 3; written before the run)
**Question**: does the 9–13 MB/s upload limit come from the crate, the process pipes, ssh.exe's stdin, or the Python fixture? No real OpenSSH server is available.

**Code**: spike branch commit ecb7215. `scratch/sftp-spike/src/sink.rs` is a native SFTP v3 sink: writes are counted and dropped, reads return a pattern. It costs one parse and one reply per request, with no disk. Upload data comes from memory (`mem=`).

**Measurements** (`run_spike.py --only diag-inproc,diag-child,diag-stdin`; 3 runs each, median):
| Id | What | Path |
|---|---|---|
| I0 | A1 upload, 1 GiB | in process: client ↔ sink over a 1 MiB in-memory duplex (no process, no OS pipe, no ssh, no Python) |
| I50 | A1 upload, 256 MiB, every reply delayed 50 ms | in process (pipelining: 64 × 32 KiB per 50 ms caps at about 42 MB/s) |
| D0 | A1 download, 1 GiB | in process |
| I0_a2 | A2 upload, 1 GiB | in process (for the record) |
| P, P_npipe, PD | A1 upload with tokio's pipes, upload with an overlapped named pipe for stdin, download | the sink as a child process (`sftp-spike serve`): the same process pipes the daemon would use, no ssh, no Python |
| S_tokio, S_thread, S_npipe, S_file | ssh.exe carrying 256 MiB of stdin to `cat > /dev/null` on the fixture: from tokio's `ChildStdin`, from a blocking thread of our own, from an overlapped named pipe, and with ssh reading a file itself | pipe-only, no SFTP |
| C | sftp CLI upload of the same 256 MiB | same run, on the fixture |

**Decision rules** (the driver prints which one fired; in order):
- **R1 → B**: I0 < 30 MB/s, or I50 < 29 MB/s (0.7 × the 42 MB/s ceiling). The crate itself cannot carry uploads even with no I/O.
- **R2** (only with I0 ≥ 100 and I50 ≥ 29): P < 70 MB/s means the process pipes are the limit.
  - **A1** if P_npipe ≥ 70, with that stdin in the phase-2 pool.
  - Otherwise **B**.
- **R3** (only with P ≥ 70): S_tokio < 60 MB/s means tokio's stdin writer into ssh.exe is the limit.
  - **A1** if another writer (thread, named pipe) reaches ≥ 60, with that writer in the pool.
  - If no writer does: no automatic verdict (the fixture's exec path may be the limit). The numbers go to the owner.
- **R4 → A1**: I0 ≥ 100, I50 ≥ 29, P ≥ 70 and S_tokio ≥ 60. Everything on Kelpie's side carries far more than the fixture takes, so the limit is the Python server.
- Anything else, between the thresholds: no automatic verdict; the numbers go to the owner.

**Stop rules**:
- Time box: 4 hours of slot time, builds included. At 4 hours I stop wherever I am and report.
- Order: build → diag-inproc → diag-child → diag-stdin. I stop as soon as a rule fires (R1 already fires after diag-inproc).
- If a mode fails to build or run, at most 45 minutes go into fixing it. After that it is dropped, and the verdict uses only what ran; a rule that needs the missing figure stays open.
- No new variants beyond those listed. No patches to either crate. Nothing outside the spike branch, apart from this report.
- Same isolation as the spike: temp folders, the fixture on 127.0.0.1, no services, only processes this run started.

## Upload diagnosis result (run 2026-11-06, 20:56–21:23, about 27 minutes of slot time)
Medians of 3 runs. Build clean, no fixes, no mode dropped.

| Id | Measurement | MB/s | Threshold |
|---|---|---|---|
| I0 | A1 upload in process, 1 GiB | **766.7** | ≥ 100 ✓ |
| D0 | A1 download in process, 1 GiB | 415.2 | |
| I50 | A1 upload in process, 50 ms simulated RTT | **38.7** | ≥ 29 ✓ (ceiling ≈ 42) |
| I0_a2 | A2 upload in process | 201.6 | |
| P | A1 upload, sink as a child process, tokio's pipes | **180.7** | ≥ 70 ✓ |
| P_npipe | the same with an overlapped named pipe for stdin | 258.3 | |
| PD | A1 download, child process | 155.9 | |
| S_tokio | ssh.exe stdin → `cat > /dev/null` on the fixture, tokio writer | **2.0** | ≥ 60 ✗ |
| S_thread | the same, blocking thread writer | 3.2 | |
| S_npipe | the same, overlapped named pipe | 3.3 | |
| S_file | the same, ssh reading a file itself (no pipe from Kelpie) | 3.8 | |
| C | sftp CLI upload on the fixture, same run | 24.2 | |

**Rule fired: R3, "no writer reaches 60 MB/s" → no automatic verdict**, as agreed.

**What the numbers show** (facts):
- R1 and R2 are cleared by wide margins:
  - russh-sftp uploads at 767 MB/s with no I/O, and keeps its 64 requests in flight at 50 ms (38.7 of the ≈ 42 possible);
  - through the very process pipes the daemon would use, it uploads at 181 MB/s (258 with the named pipe).
  - So neither the crate nor tokio's Windows process pipes come anywhere near the 9–13 MB/s seen through ssh.
- The pipe-only test cannot isolate ssh.exe. Every writer, including ssh reading a file by itself with no pipe from Kelpie, gets 2–4 MB/s, which is below even the sftp CLI's 24 MB/s on the same fixture. That limit is the fixture's exec path (paramiko → Git Bash `cat`), not ssh.exe's stdin and not the writer.

**What is still unproven**: why A1 through ssh.exe and the paramiko SFTP server uploads at about half the CLI's speed (12 vs 24 MB/s here, earlier 12 vs 23–70). The only parts left between the fast child-pipe result and the slow one are ssh.exe and the paramiko server. Telling them apart needs a non-Python SSH server, and none is available. That gap is the in-between case the rules hand back to the owner.

## Decision (owner, 2026-11-06): A1
russh-sftp over the system ssh, run in the daemon.
- **Reason**: Kelpie's side is cleared: 767 MB/s in process, and 181–258 MB/s on the daemon's own process pipes. The remaining upload gap of about 2× was seen only against the Python test server, which is most likely its cause.
- **Follow-up**: if a real OpenSSH server still shows A1 uploading about 2× slower than the `sftp` command, investigate again.
