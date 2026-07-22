# Web UI tests

The suite starts an isolated Alex daemon and `alex-fakeprov`, seeds vault accounts and traces, and drives the daemon-served UI in Chromium.

```sh
pnpm install
pnpm exec playwright install chromium
pnpm test
```

From the repository root, the installed suite can also be run with `./test.sh webui`. Set both `ALEX_BIN` and `FAKEPROV_BIN` to use prebuilt binaries; otherwise the suite incrementally builds them with Cargo.
