#!/usr/bin/env python3
import hashlib
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import make_update as mu  # noqa: E402


def fake_image(path):
    size = (mu.SYSTEM_LBA + mu.SYSTEM_SECTORS) * mu.SECTOR
    with open(path, "wb") as f:
        f.truncate(size)
        f.seek(0)
        f.write(bytes(range(256)) * 2)
        f.seek(mu.SYSTEM_LBA * mu.SECTOR)
        f.write(hashlib.sha256(b"system").digest() * 4096)


class Packaging(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = self.tmp.name
        self.key = os.path.join(self.dir, "key.pem")
        self.image = os.path.join(self.dir, "disk.img")
        mu.main(["keygen", "--key", self.key])
        fake_image(self.image)

    def tearDown(self):
        self.tmp.cleanup()

    def pack(self, *extra):
        out = os.path.join(self.dir, "out")
        rc = mu.main(
            ["pack", "--image", self.image, "--key", self.key, "--version", "0.4.0", "--build", "1790000000",
             "--out-dir", out] + list(extra)
        )
        self.assertEqual(rc, 0)
        return os.path.join(out, "deix-update-0.4.0.manifest"), os.path.join(out, "deix-update-0.4.0.payload")

    def test_inspect_summary_and_json(self):
        import contextlib
        import io

        manifest, _ = self.pack("--notes", "first build", "--op", "mkdir:/userdata/new")
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(mu.main(["inspect", "--manifest", manifest, "--key", self.key]), 0)
        text = out.getvalue()
        self.assertIn("версия:        0.4.0", text)
        self.assertIn("подпись:       верна", text)
        self.assertIn("mkdir /userdata/new", text)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(mu.main(["inspect", "--manifest", manifest, "--json"]), 0)
        doc = json.loads(out.getvalue())
        self.assertEqual(doc["manifest"]["notes"], "first build")
        self.assertEqual(len(doc["signature"]), 128)

    def test_inspect_rejects_wrong_key(self):
        manifest, _ = self.pack()
        other = os.path.join(self.dir, "other.pem")
        mu.main(["keygen", "--key", other])
        self.assertEqual(mu.main(["inspect", "--manifest", manifest, "--key", other]), 1)

    def test_roundtrip_verifies(self):
        manifest, payload = self.pack("--op", "mkdir:/userdata/new", "--op", "write:/userdata/new/a=0102")
        self.assertEqual(mu.main(["verify", "--manifest", manifest, "--payload", payload, "--key", self.key]), 0)
        _, body = mu.load_manifest(manifest)
        doc = json.loads(body)
        self.assertEqual(doc["userdata_ops"][1], {"hex": "0102", "op": "write", "path": "/userdata/new/a"})
        self.assertEqual(sum(s["sectors"] for s in doc["segments"]), 1 + 2047 + 32768)

    def test_payload_decodes_to_image_segments(self):
        manifest, payload = self.pack()
        sig, body = mu.load_manifest(manifest)
        doc = json.loads(body)
        with open(payload, "rb") as f:
            data = f.read()
        raw = b"".join(piece for _, piece in mu.iter_chunks(data, doc))
        self.assertEqual(raw, mu.read_image_segments(self.image))

    def test_tampered_manifest_rejected(self):
        manifest, payload = self.pack()
        with open(manifest, "rb") as f:
            blob = f.read()
        with open(manifest, "wb") as f:
            f.write(blob.replace(b'"channel":"stable"', b'"channel":"beta"'))
        self.assertEqual(mu.main(["verify", "--manifest", manifest, "--payload", payload, "--key", self.key]), 1)

    def test_tampered_payload_rejected(self):
        manifest, payload = self.pack()
        with open(payload, "r+b") as f:
            f.seek(20)
            b = f.read(1)
            f.seek(20)
            f.write(bytes([b[0] ^ 1]))
        self.assertEqual(mu.main(["verify", "--manifest", manifest, "--payload", payload, "--key", self.key]), 1)

    def test_other_key_rejected(self):
        manifest, payload = self.pack()
        other = os.path.join(self.dir, "other.pem")
        mu.main(["keygen", "--key", other])
        self.assertEqual(mu.main(["verify", "--manifest", manifest, "--payload", payload, "--key", other]), 1)

    def test_pubkey_file(self):
        out = os.path.join(self.dir, "pub.bin")
        mu.main(["pubkey", "--key", self.key, "--out", out])
        with open(out, "rb") as f:
            pub = f.read()
        self.assertEqual(len(pub), 65)
        self.assertEqual(pub[0], 4)
        zero = os.path.join(self.dir, "zero.bin")
        mu.main(["pubkey", "--zero", "--out", zero])
        with open(zero, "rb") as f:
            self.assertEqual(f.read(), b"\0" * 65)

    def test_bad_inputs(self):
        for bad in (["--op", "write:/etc/x=00"], ["--op", "mkdir:/userdata/../x"], ["--op", "write:/userdata/a=0"],
                    ["--notes", 'say "hi"'], ["--channel", "nightly"]):
            out = os.path.join(self.dir, "o2")
            rc = mu.main(["pack", "--image", self.image, "--key", self.key, "--version", "1", "--build", "5",
                          "--out-dir", out] + bad)
            self.assertEqual(rc, 1, bad)

    def test_der_roundtrip(self):
        for raw in (bytes(range(1, 65)), b"\xff" * 64, b"\0" * 31 + b"\1" + b"\x80" + b"\0" * 31):
            self.assertEqual(mu.der_to_raw(mu.raw_to_der(raw)), raw)


if __name__ == "__main__":
    unittest.main()
