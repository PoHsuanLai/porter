#!/usr/bin/env bash
# Regenerates the scratch CA and the leaf certificate the fake IMAP, SMTP and HTTPS servers use.
# TEST-ONLY keys: they sign nothing real, are committed on purpose, and must never be trusted by
# anything outside a test. Needs the openssl CLI (a separate process, no cert-generation crate).
# Run from anywhere; it rewrites ca.pem, leaf.pem and leaf.key next to this script.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
days=36500

openssl req -x509 -newkey rsa:2048 -nodes -keyout "$work/ca.key" -out "$work/ca.crt" \
  -days "$days" -subj "/O=porter test only/CN=porter fake CA (TEST ONLY)" \
  -addext "basicConstraints=critical,CA:TRUE,pathlen:0" \
  -addext "keyUsage=critical,keyCertSign,cRLSign" 2>/dev/null

openssl req -newkey rsa:2048 -nodes -keyout "$work/leaf.key" -out "$work/leaf.csr" \
  -subj "/O=porter test only/CN=localhost" 2>/dev/null

cat >"$work/leaf.ext" <<EXT
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=DNS:localhost,DNS:*.fake.test,DNS:imap.fake.test,DNS:smtp.fake.test,IP:127.0.0.1,IP:::1
EXT
openssl x509 -req -in "$work/leaf.csr" -CA "$work/ca.crt" -CAkey "$work/ca.key" -CAcreateserial \
  -out "$work/leaf.crt" -days "$days" -extfile "$work/leaf.ext" 2>/dev/null

label() { printf '# %s\n' "TEST-ONLY material from fixtures/regen.sh. Never trust or reuse it outside the tests."; }
{ label; cat "$work/ca.crt"; } >"$here/ca.pem"
{ label; cat "$work/leaf.crt"; } >"$here/leaf.pem"
# The leaf key as PKCS#8, which rustls-pki-types reads as a private key.
{ label; openssl pkcs8 -topk8 -nocrypt -in "$work/leaf.key"; } >"$here/leaf.key"
echo "wrote ca.pem leaf.pem leaf.key in $here"
