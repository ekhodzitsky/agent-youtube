#!/usr/bin/env python3
"""Generate public verification fixtures using OpenSSL as an independent oracle.

Private keys remain in an owned ignored runs directory, never in test fixtures.
OpenSSL is a research/fixture-generation tool, not a production dependency.
"""
import argparse
import os
from pathlib import Path
import json
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def rsa_policy(work, output, run):
    output.mkdir(exist_ok=True)
    message = b"agent-youtube RSA policy fixture\n"
    (work / "message").write_bytes(message)
    run("genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256", "-out", "leaf.key")
    run("req", "-new", "-key", "leaf.key", "-subj", "/CN=fixture.invalid", "-out", "leaf.csr")
    (work / "ext").write_text("basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:fixture.invalid\n")
    for bits in [1024, 2047, 2048, 3072]:
        key = f"rsa-{bits}.key"
        run("genpkey", "-algorithm", "RSA", "-pkeyopt", f"rsa_keygen_bits:{bits}", "-out", key)
        run("rsa", "-in", key, "-RSAPublicKey_out", "-outform", "DER", "-out", str(output / f"{bits}.pub"))
        for padding in ["pkcs1", "pss"]:
            for digest in ["sha256", "sha384", "sha512"]:
                # A digest-sized SHA512 PSS salt cannot fit in RSA-1024.
                if (bits, padding, digest) == (1024, "pss", "sha512"):
                    continue
                options = ["-sigopt", "rsa_padding_mode:pss", "-sigopt", "rsa_pss_saltlen:digest"] if padding == "pss" else []
                run("dgst", "-" + digest, "-sign", key, *options, "-out", str(output / f"{bits}-{padding}-{digest}.sig"), "message")
        ca = f"ca-{bits}.pem"
        run("req", "-new", "-x509", "-key", key, "-subj", "/CN=Owned RSA policy CA", "-days", "365",
            "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign", "-out", ca)
        run("x509", "-in", ca, "-outform", "DER", "-out", str(output / f"ca-{bits}.der"))
        run("x509", "-req", "-in", "leaf.csr", "-CA", ca, "-CAkey", key, "-set_serial", str(bits),
            "-days", "365", "-extfile", "ext", "-outform", "DER", "-out", str(output / f"leaf-{bits}.der"))
    (output / "parameters.json").write_text(json.dumps({"valid_at": int(time.time()) + 60, "message": message.decode()}, indent=2) + "\n")
    for path in output.iterdir():
        if path.is_file():
            path.chmod(0o644)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rsa-policy', action='store_true', help='generate only RSA strength-policy boundary fixtures')
    args = parser.parse_args()
    os.umask(0o077)
    work = Path(tempfile.mkdtemp(prefix="tls-fixtures-", dir=ROOT / "runs"))
    output = ROOT / "tests/fixtures/tls"
    output.mkdir(exist_ok=True)

    def run(*args):
        subprocess.run(["openssl", *args], cwd=work, check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    if args.rsa_policy:
        rsa_policy(work, output / "rsa-policy", run)
        print("Public RSA policy fixtures generated; private keys remain in owned ignored scratch")
        return

    message = b"agent-youtube public TLS verification fixture\n"
    (work / "message").write_bytes(message)
    run("genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", "rsa.pem")
    for name, curve in [("p256", "prime256v1"), ("p384", "secp384r1")]:
        run("genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:" + curve, "-out", name + ".pem")
        run("pkey", "-in", name + ".pem", "-pubout", "-outform", "DER", "-out", name + ".der")
        # SEC1 point is the final uncompressed point of these fixed SPKI encodings.
        size = 65 if name == "p256" else 97
        (output / (name + ".pub")).write_bytes((work / (name + ".der")).read_bytes()[-size:])
    run("genpkey", "-algorithm", "ED25519", "-out", "ed25519.pem")
    run("pkey", "-in", "ed25519.pem", "-pubout", "-outform", "DER", "-out", "ed25519.der")
    (output / "ed25519.pub").write_bytes((work / "ed25519.der").read_bytes()[-32:])
    run("rsa", "-in", "rsa.pem", "-RSAPublicKey_out", "-outform", "DER", "-out", str(output / "rsa.pub"))
    cases = [("p256", "sha256", False), ("p256", "sha384", False),
             ("p384", "sha256", False), ("p384", "sha384", False), ("ed25519", None, False)]
    cases += [("rsa", digest, pss) for pss in [False, True] for digest in ["sha256", "sha384", "sha512"]]
    for index, (key, digest, pss) in enumerate(cases):
        signature = str(output / f"{index}.sig")
        if key == "ed25519":
            run("pkeyutl", "-sign", "-rawin", "-inkey", "ed25519.pem", "-in", "message", "-out", signature)
        else:
            options = ["-sigopt", "rsa_padding_mode:pss", "-sigopt", "rsa_pss_saltlen:digest"] if pss else []
            run("dgst", "-" + digest, "-sign", key + ".pem", *options, "-out", signature, "message")
    run("req", "-x509", "-new", "-key", "rsa.pem", "-sha256", "-days", "3650", "-subj", "/CN=agent-youtube test CA",
        "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign,cRLSign", "-out", "ca.pem")
    run("req", "-new", "-key", "p256.pem", "-subj", "/CN=fixture.invalid", "-out", "leaf.csr")
    (work / "ext").write_text("subjectAltName=DNS:fixture.invalid\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n")
    run("x509", "-req", "-in", "leaf.csr", "-CA", "ca.pem", "-CAkey", "rsa.pem", "-CAcreateserial", "-sha256",
        "-days", "365", "-extfile", "ext", "-out", "leaf.pem")
    for name in ["ca", "leaf"]:
        run("x509", "-in", name + ".pem", "-outform", "DER", "-out", str(output / (name + ".der")))
    (output / "parameters.json").write_text(json.dumps({"valid_at": int(time.time()) + 86400, "message": message.decode()}, indent=2) + "\n")
    print("Public fixtures generated; private keys retained only in owned ignored scratch")


if __name__ == "__main__":
    main()
