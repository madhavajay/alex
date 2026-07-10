# Alexandria macOS PR test bundle

This archive contains the exact pull-request build of:

- `AlexandriaBar.app`, ad-hoc signed with the production bundle identifier;
- the release-mode `alex` daemon/CLI; and
- the equivalent `alexandria` CLI binary.

It is intentionally **not Developer ID signed or notarized**. It contains no
credentials, account files, or developer-machine configuration.

## Install and test

From Terminal, in the extracted bundle directory:

```bash
./install.sh
```

The script stops the current menu app, saves the first app and CLI builds it
replaces, clears quarantine from this ad-hoc build, installs the CLI under
`~/.local/bin`, registers/restarts Alexandria's launchd user service, and opens
the app. Existing data under `~/.alexandria` is left untouched.

The app uses the same bundle identifier as production, so only one
`AlexandriaBar.app` should be running. macOS may request an administrator
password when replacing `/Applications/AlexandriaBar.app`.

Confirm the daemon after installation:

```bash
~/.local/bin/alex status
curl -fsS http://127.0.0.1:4100/health
```

`BUILD_INFO.txt` records the pull request, commit, architecture, bundle ID, and
version represented by the archive.

## Switch back

Keep this extracted directory until testing is finished, then run:

```bash
./install.sh --restore
```

That restores the app and CLI binaries saved by the first CI install and
re-registers the daemon service. If there was no prior CLI to save, reinstall
the production CLI with Homebrew instead.
