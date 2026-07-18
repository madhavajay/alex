# Signed macOS Build

This repo uses `build-signed.sh` to build `Alex.app`, sign it with a
Developer ID Application certificate, package it into a DMG, and optionally
notarize/staple the DMG.

The bundle identifier is:

```text
com.madhavajay.alex
```

## Local Setup

Fill in `.env` locally. The file is ignored by git and has empty placeholders.

Required for signing:

```sh
APPLE_SIGNING_IDENTITY=
SIGNING_CERTIFICATE_P12_DATA=
SIGNING_CERTIFICATE_PASSWORD=
KEYCHAIN_PASSWORD=
```

Required for notarization:

```sh
APPLE_ID=
APPLE_PASSWORD=
APPLE_TEAM_ID=
```

If the Developer ID Application certificate is already in your keychain,
`APPLE_SIGNING_IDENTITY` can be discovered automatically.

To build:

```sh
./build-signed.sh
```

To force notarization and fail if credentials are missing:

```sh
./build-signed.sh --notarize
```

To validate the result:

```sh
./check-gatekeeper.sh macos/dist/*.dmg
```

To install the latest generated DMG:

```sh
./install-dmg.sh
```

## GitHub Secrets

The GitHub workflow expects secrets with the same names as the `.env` keys:

```text
APPLE_ID
APPLE_PASSWORD
APPLE_TEAM_ID
APPLE_SIGNING_IDENTITY
SIGNING_CERTIFICATE_P12_DATA
SIGNING_CERTIFICATE_PASSWORD
KEYCHAIN_PASSWORD
```

`SIGNING_CERTIFICATE_P12_DATA` should be the base64 encoded `.p12` export of
the Developer ID Application certificate.
