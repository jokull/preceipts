#!/usr/bin/env bash
# Create a self-signed code-signing certificate so sign.sh can be run without an
# Apple Developer account. Adapted from the Swift-era Scripts/setup_dev_signing.sh.
#
# This exists to exercise the packaging path, not to ship anything. What a
# self-signed cert gives you is a stable code identity, which is enough to stop
# the keychain re-prompting on every rebuild. What it cannot give you is a Team
# ID, and every entitlement that matters here is namespaced by one — see the
# closing notes.
set -euo pipefail

APP_NAME=${APP_NAME:-Preceipts}
CERT_NAME=${CERT_NAME:-"${APP_NAME} Development"}

notes() {
  cat <<MSG

  export APP_IDENTITY='$CERT_NAME'

What this certificate does NOT do:

  * It has no Team ID, so it grants no keychain access group. The app and
    preceiptsd cannot share a keychain item under one; sign.sh drops the
    entitlement rather than embedding one that would never match. Sharing
    secrets between them needs a real Team ID from Apple.
  * It is not a Developer ID, so the bundle cannot be notarized and Gatekeeper
    will refuse it on any machine but this one.

Use it to check that the bundle assembles, signs, and verifies. Nothing beyond
that is being tested here, and nothing signed this way should leave the machine.
MSG
}

if security find-certificate -c "$CERT_NAME" >/dev/null 2>&1; then
  echo "Certificate '$CERT_NAME' already exists."
  notes
  exit 0
fi

echo "Creating self-signed certificate '$CERT_NAME'..."

# openssl needs the code-signing EKU explicitly; without it the cert imports
# fine and codesign then refuses to use it, which reads as a keychain problem.
TEMP_DIR=$(mktemp -d)
trap 'rm -rf "$TEMP_DIR"' EXIT

cat > "$TEMP_DIR/openssl.cnf" <<EOFCONF
[ req ]
distinguished_name = req_distinguished_name
x509_extensions = v3_req
prompt = no

[ req_distinguished_name ]
CN = $CERT_NAME
O = ${APP_NAME} Development
C = US

[ v3_req ]
keyUsage = critical,digitalSignature
extendedKeyUsage = codeSigning
EOFCONF

openssl req -x509 -newkey rsa:4096 -sha256 -days 3650 \
    -nodes -keyout "$TEMP_DIR/dev.key" -out "$TEMP_DIR/dev.crt" \
    -config "$TEMP_DIR/openssl.cnf" 2>/dev/null

openssl pkcs12 -export -out "$TEMP_DIR/dev.p12" \
    -inkey "$TEMP_DIR/dev.key" -in "$TEMP_DIR/dev.crt" \
    -passout pass: 2>/dev/null

# -T lets codesign use the key without a prompt per invocation. That is the
# whole convenience being bought here.
security import "$TEMP_DIR/dev.p12" -k ~/Library/Keychains/login.keychain-db \
  -T /usr/bin/codesign -T /usr/bin/security

echo
echo "Imported. Now mark it 'Always Trust' for Code Signing in Keychain Access —"
echo "an untrusted cert signs, and then verification refuses the result."
notes
