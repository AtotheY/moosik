#!/bin/bash
# One-time setup for this user's local Moosik builds. No Apple account required.
set -euo pipefail
umask 077
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
name="Moosik Local Development"
keychain="$HOME/Library/Keychains/login.keychain-db"
config="$root/.ark-build/signing-identity"
[ "$(uname -s)" = Darwin ] || { echo "This setup requires macOS." >&2; exit 1; }

# Never silently replace an existing certificate: changing it would lose trust.
fingerprint="$(security find-certificate -c "$name" -Z "$keychain" 2>/dev/null | awk '/SHA-1 hash:/ { print $3; exit }' || true)"
if [ -z "$fingerprint" ]; then
    work="$(mktemp -d "${TMPDIR:-/tmp}/moosik-signing.XXXXXX")"
    trap 'rm -rf "$work"' EXIT
    cat > "$work/certificate.cnf" <<'CONFIG'
[req]
distinguished_name = subject
x509_extensions = codesign
prompt = no
[subject]
CN = Moosik Local Development
[codesign]
basicConstraints = critical,CA:FALSE
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
subjectKeyIdentifier = hash
CONFIG
    openssl req -new -x509 -newkey rsa:3072 -nodes -sha256 -days 3650 \
        -config "$work/certificate.cnf" -keyout "$work/private.pem" \
        -out "$work/certificate.pem" 2>"$work/openssl.log" || {
        cat "$work/openssl.log" >&2; exit 1;
    }
    # Use PKCS#12 algorithms supported by macOS's importer as well as OpenSSL 3.
    openssl rand -hex 32 > "$work/import-password"
    openssl pkcs12 -export -inkey "$work/private.pem" -in "$work/certificate.pem" \
        -name "$name" -out "$work/identity.p12" -passout "file:$work/import-password" \
        -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1
    # Only codesign gets automatic access; the imported private key is non-exportable.
    security import "$work/identity.p12" -k "$keychain" \
        -P "$(cat "$work/import-password")" -x -T /usr/bin/codesign
    fingerprint="$(security find-certificate -c "$name" -Z "$keychain" | awk '/SHA-1 hash:/ { print $3; exit }')"
fi
[ -n "$fingerprint" ] || { echo "Could not find the local certificate." >&2; exit 1; }

# Scope trust to code signing, in the current user's domain only. macOS may ask
# for authentication here. Never change trust for SSL, the system, or session items.
if ! security find-identity -v -p codesigning "$keychain" | grep -Fq "$fingerprint"; then
    work="${work:-$(mktemp -d "${TMPDIR:-/tmp}/moosik-signing.XXXXXX")}"
    trap 'rm -rf "$work"' EXIT
    security find-certificate -c "$name" -p "$keychain" > "$work/certificate.pem"
    security add-trusted-cert -r trustRoot -p codeSign -k "$keychain" "$work/certificate.pem"
fi
security find-identity -v -p codesigning "$keychain" | grep -Fq "$fingerprint" || {
    echo "The local signing identity is not valid; configuration was not changed." >&2; exit 1;
}
mkdir -p "$(dirname "$config")"
printf '%s\n' "$fingerprint" > "$config"
printf 'Moosik will reuse %s (%s) for local builds.\n' "$name" "$fingerprint"
