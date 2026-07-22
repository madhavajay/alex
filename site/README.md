# Alex public site

The public site is a dependency-free static build deployed to GitHub Pages.

```sh
cd site
npm ci
npm test
npm run build
```

The current landing page is built from `src/` and published at the site root.
The previous interactive concept is preserved as a static snapshot in `old/`
and published at `/old/`.

The Pages workflow deploys `site/dist/` to `gh-pages` while preserving the
stable and beta Sparkle appcasts already hosted on that branch.
