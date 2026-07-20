# Alex public site

This directory is a dependency-free static GitHub Pages site. Its first
walkthrough is generated from the same deterministic Fable-to-Sol middleware
fixture used by the proxy tests.

```sh
cd site
npm ci
npm test
npm run build
```

The output is written to `site/dist/`. The build manifest contains SHA-256
hashes of every published file, so two builds of one commit can be compared
byte for byte.

## Analytics contract

`src/analytics-schema.js` is the complete event and property allowlist. The
browser discards undeclared properties and respects Global Privacy Control and
Do Not Track. Events are sent to Plausible's cookie-free event endpoint and
also emitted as `alex:analytics` DOM events for deterministic tests and local
inspection.

Only page identity, demo/provider identifiers, UI surfaces, step counts, and
campaign attribution are accepted. Prompts, traces, credentials, request
bodies, response bodies, full referrers, and arbitrary URLs cannot enter an
event payload.

## Pages deployment

`.github/workflows/pages.yml` builds and tests on every relevant pull request.
On `main`, it deploys the exact build artifact to the existing `gh-pages`
branch. The deploy shares the `gh-pages-deploy` concurrency group with the
release appcast workflow and explicitly preserves `appcast.xml` and
`appcast-beta.xml`.
