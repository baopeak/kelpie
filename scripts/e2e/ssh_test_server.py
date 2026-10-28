"""A small SSH server for Kelpie's remote-project tests (needs `paramiko`).

    python ssh_test_server.py <port> <authorized-public-key-file> <host-key-file>

Logs in with the given public key (or one in $HOME/.ssh/authorized_keys), or with a password when
KELPIE_TEST_SSH_PASSWORD is set. KELPIE_TEST_HOME replaces $HOME for commands and SFTP, so tests that
change ~/.ssh on the "remote" never touch the real one.
Every command runs through Git Bash, so the "remote" is this machine behaving like a POSIX host;
a pty request is accepted (output is unbuffered, but it is not a real terminal: fine for running
commands and agents' plain output). Also serves SFTP (what `scp` speaks), local / dynamic port
forwards (direct-tcpip) and remote forwards (tcpip-forward). Paths are Git Bash style
(`/c/Users/...`). Not for any use but tests: no sandboxing, binds to 127.0.0.1.
"""
import os
import re
import socket
import subprocess
import sys
import threading

import paramiko

PORT = int(sys.argv[1])
AUTH_LINE = open(sys.argv[2]).read().split()[1]
HOST_KEY_FILE = sys.argv[3]
PASSWORD = os.environ.get("KELPIE_TEST_SSH_PASSWORD")
BASH = os.environ.get("KELPIE_TEST_BASH", r"C:\Program Files\Git\bin\bash.exe")
HOME = os.environ.get("KELPIE_TEST_HOME") or os.path.expanduser("~")

if not os.path.exists(HOST_KEY_FILE):
    paramiko.RSAKey.generate(2048).write_private_key_file(HOST_KEY_FILE)
host_key = paramiko.RSAKey(filename=HOST_KEY_FILE)


_paths = {}


def local_path(p):
    """`/c/Users/x` or `/tmp/x` → a Windows path (Git Bash paths as the remote shell prints them)."""
    if p not in _paths:
        m = re.match(r"^/([a-zA-Z])(/.*)?$", p)
        _paths[p] = f"{m.group(1).upper()}:{m.group(2) or '/'}" if m else subprocess.run([BASH, "-c", 'cygpath -w "$1"', "_", p], capture_output=True, text=True).stdout.strip() or p
    return _paths[p]


class Server(paramiko.ServerInterface):
    def __init__(self, transport):
        self.transport = transport
        self.commands = {}  # chanid -> command (None: shell)
        self.ptys = set()  # chanids that asked for a terminal
        self.ready = {}  # chanid -> Event
        self.tcpip = {}  # chanid -> (host, port) for direct-tcpip
        self.listeners = {}  # (addr, port) -> socket for remote forwards

    def check_auth_publickey(self, username, key):
        try:
            extra = [line.split()[1] for line in open(os.path.join(HOME, ".ssh", "authorized_keys")) if len(line.split()) > 1]
        except OSError:
            extra = []
        return paramiko.AUTH_SUCCESSFUL if key.get_base64() in [AUTH_LINE, *extra] else paramiko.AUTH_FAILED

    def check_auth_password(self, username, password):
        return paramiko.AUTH_SUCCESSFUL if PASSWORD and password == PASSWORD else paramiko.AUTH_FAILED

    def get_allowed_auths(self, username):
        return "publickey,password" if PASSWORD else "publickey"

    def check_channel_request(self, kind, chanid):
        if kind == "session":
            self.ready[chanid] = threading.Event()
            return paramiko.OPEN_SUCCEEDED
        return paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

    def check_channel_direct_tcpip_request(self, chanid, origin, destination):
        self.tcpip[chanid] = destination
        return paramiko.OPEN_SUCCEEDED

    def check_channel_pty_request(self, channel, *args):
        self.ptys.add(channel.get_id())
        return True

    def check_channel_env_request(self, channel, name, value):
        return True

    def check_channel_exec_request(self, channel, command):
        self.commands[channel.get_id()] = command.decode()
        self.ready[channel.get_id()].set()
        return True

    def check_channel_shell_request(self, channel):
        self.commands[channel.get_id()] = None
        self.ready[channel.get_id()].set()
        return True

    def check_port_forward_request(self, address, port):
        s = socket.socket()
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        s.bind(("127.0.0.1", port))
        s.listen(10)
        port = s.getsockname()[1]
        self.listeners[(address, port)] = s

        def accept():
            while True:
                try:
                    c, origin = s.accept()
                except OSError:
                    return
                chan = self.transport.open_forwarded_tcpip_channel(origin, (address, port))
                splice(c, chan)

        threading.Thread(target=accept, daemon=True).start()
        return port

    def cancel_port_forward_request(self, address, port):
        s = self.listeners.pop((address, port), None)
        if s:
            s.close()


def splice(sock, chan):
    """Copies both ways between a TCP socket and an SSH channel until either closes."""
    def one(src, dst):
        try:
            while True:
                data = src.recv(65536)
                if not data:
                    break
                dst.sendall(data)
        except Exception:
            pass
        for x in (sock, chan):
            try:
                x.close()
            except Exception:
                pass

    threading.Thread(target=one, args=(sock, chan), daemon=True).start()
    threading.Thread(target=one, args=(chan, sock), daemon=True).start()


class Handle(paramiko.SFTPHandle):
    def stat(self):
        try:
            return paramiko.SFTPAttributes.from_stat(os.fstat(self.readfile.fileno()))
        except OSError as e:
            return paramiko.SFTPServer.convert_errno(e.errno)

    def chattr(self, attr):
        return paramiko.SFTP_OK


class Sftp(paramiko.SFTPServerInterface):
    """The local file system through Git Bash style paths."""

    def canonicalize(self, path):
        if path in ("", "."):
            path = subprocess.run([BASH, "-c", "printf %s \"$HOME\""], capture_output=True, text=True, env=dict(os.environ, HOME=HOME)).stdout or "/"
            if re.match(r"^[A-Za-z]:", path):
                path = "/" + path[0].lower() + path[2:].replace("\\", "/")
        return os.path.normpath(path).replace("\\", "/") if path.startswith("/") else path

    def _err(self, e):
        return paramiko.SFTPServer.convert_errno(e.errno)

    def list_folder(self, path):
        try:
            out = []
            for name in os.listdir(local_path(path)):
                attr = paramiko.SFTPAttributes.from_stat(os.stat(os.path.join(local_path(path), name)))
                attr.filename = name
                out.append(attr)
            return out
        except OSError as e:
            return self._err(e)

    def stat(self, path):
        try:
            return paramiko.SFTPAttributes.from_stat(os.stat(local_path(path)))
        except OSError as e:
            return self._err(e)

    lstat = stat

    def open(self, path, flags, attr):
        p = local_path(path)
        try:
            fd = os.open(p, flags | getattr(os, "O_BINARY", 0), 0o666)
            mode = "ab" if flags & os.O_APPEND else ("r+b" if flags & os.O_RDWR else ("wb" if flags & os.O_WRONLY else "rb"))
            f = os.fdopen(fd, mode)
        except OSError as e:
            return self._err(e)
        h = Handle(flags)
        h.filename = p
        h.readfile = f
        h.writefile = f
        return h

    def remove(self, path):
        try:
            os.remove(local_path(path))
        except OSError as e:
            return self._err(e)
        return paramiko.SFTP_OK

    def rename(self, old, new):
        try:
            os.rename(local_path(old), local_path(new))
        except OSError as e:
            return self._err(e)
        return paramiko.SFTP_OK

    posix_rename = rename

    def mkdir(self, path, attr):
        try:
            os.mkdir(local_path(path))
        except OSError as e:
            return self._err(e)
        return paramiko.SFTP_OK

    def rmdir(self, path):
        try:
            os.rmdir(local_path(path))
        except OSError as e:
            return self._err(e)
        return paramiko.SFTP_OK

    def chattr(self, path, attr):
        return paramiko.SFTP_OK


class SftpSubsystem(paramiko.SFTPServer):
    """paramiko closes an SFTP channel without an exit status; OpenSSH's scp then reports a
    failure. Real servers send 0."""

    def start_subsystem(self, name, transport, channel):
        self._chan = channel
        super().start_subsystem(name, transport, channel)

    def finish_subsystem(self):
        try:
            self._chan.send_exit_status(0)
        except Exception:
            pass
        super().finish_subsystem()


def pump(src, chan, stderr=False):
    try:
        while True:
            data = src.read1(65536) if hasattr(src, "read1") else src.read(65536)
            if not data:
                break
            (chan.sendall_stderr if stderr else chan.sendall)(data)
    except Exception:
        pass


def run_session(server, chan):
    if not server.ready[chan.get_id()].wait(10):
        return
    command = server.commands.get(chan.get_id())
    argv = [BASH, "-c", command] if command else [BASH, "-i"]
    env = dict(os.environ, HOME=HOME, TERM="xterm-256color", SHELL="/usr/bin/bash")
    p = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, bufsize=0)
    threads = [threading.Thread(target=pump, args=(p.stdout, chan)), threading.Thread(target=pump, args=(p.stderr, chan, True))]
    for th in threads:
        th.daemon = True
        th.start()

    def feed():
        try:
            while True:
                data = chan.recv(65536)
                if not data:
                    break
                # A terminal turns Enter (CR) into a line feed; a command run without one gets its input as it is.
                p.stdin.write(data.replace(b"\r", b"\n") if chan.get_id() in server.ptys else data)
                p.stdin.flush()
        except Exception:
            pass
        try:
            p.stdin.close()
        except Exception:
            pass

    threading.Thread(target=feed, daemon=True).start()
    code = p.wait()
    for th in threads:
        th.join(2)
    try:
        chan.send_exit_status(code)
        chan.close()
    except Exception:
        pass


def serve(client):
    t = paramiko.Transport(client)
    t.add_server_key(host_key)
    t.set_subsystem_handler("sftp", SftpSubsystem, Sftp)
    server = Server(t)
    try:
        t.start_server(server=server)
    except Exception:
        return
    # Channels until the client goes away: sessions run commands (an SFTP session is handled by
    # paramiko's subsystem thread), direct-tcpip channels connect to their destination.
    while t.is_active():
        chan = t.accept(1)
        if chan is None:
            continue
        dest = server.tcpip.get(chan.get_id())
        if dest:
            try:
                splice(socket.create_connection(dest, timeout=10), chan)
            except OSError:
                chan.close()
            continue
        if server.ready.get(chan.get_id()) is not None:
            def session(c=chan):
                # A subsystem request (sftp) never sets `ready`: paramiko runs it itself.
                server.ready[c.get_id()].wait(2)
                if c.get_id() in server.commands:
                    run_session(server, c)
            threading.Thread(target=session, daemon=True).start()
    t.close()


sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock.bind(("127.0.0.1", PORT))
sock.listen(50)
print(f"ssh test server on 127.0.0.1:{PORT}", flush=True)
while True:
    c, _ = sock.accept()
    threading.Thread(target=serve, args=(c,), daemon=True).start()
