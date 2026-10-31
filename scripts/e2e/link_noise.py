"""The phone's side of kelpie-link for the e2e scripts: Noise IKpsk1 pairing and IK reconnect
(25519 / ChaChaPoly / BLAKE2s), the first message's header (version, mode, mac1) and the session framing of
crates/kelpie-link (chunks with a flags byte, rekey by counting messages). It is checked against the vectors the Rust crate writes
(`crates/kelpie-link/tests/vectors.json`): `python link_noise.py` runs that check.

Uses the `cryptography` package for X25519 and ChaCha20-Poly1305, and hashlib for BLAKE2s.
"""
import base64
import hashlib
import hmac
import json
import os
import struct
import sys

from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey, X25519PublicKey
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305
from cryptography.hazmat.primitives import serialization

PAIR = 1
RECONNECT = 2
VERSION = 1
PROLOGUE = b"kelpie-link/%d" % VERSION
HEADER = 18  # version, mode, mac1
MAX_NOISE = 65535
TAG = 16
MAX_CHUNK = MAX_NOISE - TAG - 1
MAX_APP = 1 << 20
REKEY_EVERY = 1 << 16
MORE = 1
HASHLEN = 32
RAW = serialization.Encoding.Raw


def b64(b):
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def unb64(s):
    return base64.urlsafe_b64decode(s + "=" * (-len(s) % 4))


def _h(data):
    return hashlib.blake2s(data).digest()


def _hmac(key, data):
    return hmac.new(key, data, hashlib.blake2s).digest()


def _hkdf(ck, ikm, n):
    temp = _hmac(ck, ikm)
    out, prev = [], b""
    for i in range(1, n + 1):
        prev = _hmac(temp, prev + bytes([i]))
        out.append(prev)
    return out


def keypair(private=None):
    k = X25519PrivateKey.from_private_bytes(private) if private else X25519PrivateKey.generate()
    return k.private_bytes(RAW, serialization.PrivateFormat.Raw, serialization.NoEncryption()), k.public_key().public_bytes(RAW, serialization.PublicFormat.Raw)


def _dh(private, public):
    return X25519PrivateKey.from_private_bytes(private).exchange(X25519PublicKey.from_public_bytes(public))


class _Cipher:
    def __init__(self, k=None):
        self.k, self.n = k, 0

    @staticmethod
    def _nonce(n):
        return b"\x00" * 4 + struct.pack("<Q", n)

    def encrypt(self, ad, p):
        if self.k is None:
            return p
        c = ChaCha20Poly1305(self.k).encrypt(self._nonce(self.n), p, ad)
        self.n += 1
        return c

    def decrypt(self, ad, c):
        if self.k is None:
            return c
        p = ChaCha20Poly1305(self.k).decrypt(self._nonce(self.n), c, ad)
        self.n += 1
        return p

    def rekey(self):
        self.k = ChaCha20Poly1305(self.k).encrypt(self._nonce(2**64 - 1), b"\x00" * 32, b"")[:32]


class _Symmetric:
    def __init__(self, name):
        name = name.encode()
        self.h = name.ljust(HASHLEN, b"\x00") if len(name) <= HASHLEN else _h(name)
        self.ck = self.h
        self.c = _Cipher()

    def mix_hash(self, data):
        self.h = _h(self.h + data)

    def mix_key(self, ikm):
        self.ck, k = _hkdf(self.ck, ikm, 2)
        self.c = _Cipher(k[:32])

    def mix_key_and_hash(self, ikm):
        self.ck, th, k = _hkdf(self.ck, ikm, 3)
        self.mix_hash(th)
        self.c = _Cipher(k[:32])

    def encrypt_and_hash(self, p):
        c = self.c.encrypt(self.h, p)
        self.mix_hash(c)
        return c

    def decrypt_and_hash(self, c):
        p = self.c.decrypt(self.h, c)
        self.mix_hash(c)
        return p

    def split(self):
        k1, k2 = _hkdf(self.ck, b"", 2)
        return _Cipher(k1[:32]), _Cipher(k2[:32])


def mac1(key_material, label, version, mode, msg):
    """The first message's prefilter: keyed BLAKE2s-128, keyed from the desktop's public key or the pairing secret."""
    key = _h(label + key_material)
    return hashlib.blake2s(bytes([version, mode]) + msg, digest_size=16, key=key).digest()


def pair_code(hash_):
    """The six digits: HKDF-BLAKE2s(ikm = handshake hash, no salt, info = "kelpie-sas"), 4 bytes big-endian mod 10^6."""
    prk = _hmac(b"\x00" * 32, hash_)
    t1 = _hmac(prk, b"kelpie-sas\x01")
    return "%06d" % (int.from_bytes(t1[:4], "big") % 1_000_000)


class Initiator:
    """`first` is the first socket message; `finish(reply)` gives the Session."""

    def __init__(self, mode, s_priv, s_pub, desktop_pub, psk=None, ephemeral=None, payload=b"", prologue=None):
        """`payload` and `prologue` exist for the published test vectors only: kelpie-link's first message carries nothing."""
        assert (mode == PAIR) == (psk is not None)
        self.psk_mode = psk is not None
        name = "Noise_IKpsk1_25519_ChaChaPoly_BLAKE2s" if self.psk_mode else "Noise_IK_25519_ChaChaPoly_BLAKE2s"
        self.ss = _Symmetric(name)
        self.ss.mix_hash(PROLOGUE + bytes([mode]) if prologue is None else prologue)
        self.s_priv, self.rs = s_priv, desktop_pub
        self.ss.mix_hash(desktop_pub)
        self.e_priv, e_pub = keypair(ephemeral)
        msg = e_pub
        self.ss.mix_hash(e_pub)
        if self.psk_mode:
            self.ss.mix_key(e_pub)
        self.ss.mix_key(_dh(self.e_priv, desktop_pub))  # es
        msg += self.ss.encrypt_and_hash(s_pub)  # s
        self.ss.mix_key(_dh(s_priv, desktop_pub))  # ss
        if self.psk_mode:
            self.ss.mix_key_and_hash(psk)
        msg += self.ss.encrypt_and_hash(payload)
        self.noise1 = msg
        tag = mac1(psk, b"kelpie-link mac1 psk", VERSION, mode, msg) if psk is not None else mac1(desktop_pub, b"kelpie-link mac1 desktop", VERSION, mode, msg)
        self.first = bytes([VERSION, mode]) + tag + msg

    def finish(self, reply, payload_allowed=False):
        re, rest = reply[:32], reply[32:]
        self.ss.mix_hash(re)
        if self.psk_mode:
            self.ss.mix_key(re)
        self.ss.mix_key(_dh(self.e_priv, re))  # ee
        self.ss.mix_key(_dh(self.s_priv, re))  # se
        payload = self.ss.decrypt_and_hash(rest)
        if payload and not payload_allowed:
            raise ValueError("a reply that carries anything is not kelpie-link")
        send, recv = self.ss.split()
        return Session(send, recv, self.ss.h), payload


class Session:
    """seal(msg) -> socket messages; open(frame) -> the whole message once its last chunk is in, else None."""

    def __init__(self, send, recv, hash_):
        self.send, self.recv, self.hash = send, recv, hash_
        self.sent_since_rekey = 0
        self.received_since_rekey = 0
        self.rekey_every = REKEY_EVERY
        self.partial = b""

    def pair_code(self):
        return pair_code(self.hash)

    def seal(self, msg):
        if isinstance(msg, str):
            msg = msg.encode()
        assert len(msg) <= MAX_APP
        chunks = [msg[i:i + MAX_CHUNK] for i in range(0, len(msg), MAX_CHUNK)] or [b""]
        out = []
        for i, chunk in enumerate(chunks):
            flags = MORE if i < len(chunks) - 1 else 0
            out.append(self.send.encrypt(b"", bytes([flags]) + chunk))
            self.sent_since_rekey += 1
            if self.sent_since_rekey >= self.rekey_every:
                self.send.rekey()
                self.sent_since_rekey = 0
        return out

    def open(self, frame):
        plain = self.recv.decrypt(b"", frame)
        self.received_since_rekey += 1
        if self.received_since_rekey >= self.rekey_every:
            self.recv.rekey()
            self.received_since_rekey = 0
        flags, chunk = plain[0], plain[1:]
        if flags & ~MORE:
            raise ValueError("bad flags")
        self.partial += chunk
        if len(self.partial) > MAX_APP:
            raise ValueError("too large")
        if flags & MORE:
            return None
        whole, self.partial = self.partial, b""
        return whole


def check_vectors(path=None):
    """Replays the Rust crate's vectors: same first message, same session keys, same code."""
    path = path or os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "crates", "kelpie-link", "tests", "vectors.json")
    v = json.load(open(path, encoding="utf-8"))
    x = bytes.fromhex
    _, phone_pub = keypair(x(v["phone_private"]))
    assert phone_pub.hex() == v["phone_public"]
    for name, mode in (("pair", PAIR), ("reconnect", RECONNECT)):
        c = v[name]
        init = Initiator(mode, x(v["phone_private"]), phone_pub, x(v["desktop_public"]), x(v["psk"]) if mode == PAIR else None, x(v["phone_ephemeral"]))
        assert init.first.hex() == c["first"], name + ": first message"
        session, payload = init.finish(x(c["reply"]))
        assert payload == b""
        assert session.hash.hex() == c["hash"], name + ": hash"
        assert session.pair_code() == c["code"], name + ": code"
        session.rekey_every = c["rekey_every"]
        for frame, plain in zip(c["to_phone"], c["to_phone_plain"]):
            assert session.open(x(frame)) == plain.encode(), name + ": to phone"
        for frame, plain in zip(c["to_desktop"], c["to_desktop_plain"]):
            assert session.seal(plain)[0].hex() == frame, name + ": to desktop"
    return True


def check_cacophony(path=None):
    """The published Noise_IK / Noise_IKpsk1 (25519, ChaChaPoly, BLAKE2s) vectors: the same state machine, raw transport."""
    path = path or os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "crates", "kelpie-link", "tests", "cacophony-ik.json")
    x = bytes.fromhex
    for v in json.load(open(path, encoding="utf-8"))["vectors"]:
        psk = x(v["init_psks"][0]) if v.get("init_psks") else None
        _, s_pub = keypair(x(v["init_static"]))
        m = v["messages"]
        init = Initiator(PAIR if psk else RECONNECT, x(v["init_static"]), s_pub, x(v["init_remote_static"]), psk, x(v["init_ephemeral"]), x(m[0]["payload"]), x(v["init_prologue"]))
        name = v["protocol_name"]
        assert init.noise1.hex() == m[0]["ciphertext"], name + ": message 1"
        session, payload = init.finish(x(m[1]["ciphertext"]), payload_allowed=True)
        assert payload.hex() == m[1]["payload"], name + ": message 2"
        assert session.hash.hex() == v["handshake_hash"], name + ": hash"
        for i, msg in enumerate(m[2:]):
            if i % 2 == 0:
                assert session.send.encrypt(b"", x(msg["payload"])).hex() == msg["ciphertext"], name
            else:
                assert session.recv.decrypt(b"", x(msg["ciphertext"])).hex() == msg["payload"], name
    return True


if __name__ == "__main__":
    check_vectors()
    check_cacophony()
    print("kelpie-link vectors and the published Noise IK / IKpsk1 vectors: ok")
