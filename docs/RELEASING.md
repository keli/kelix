# Releasing

Maintainer guide for cutting a release and updating distribution channels.

## 1. Create and push a tag

`Release Bundles` and `Release Container` workflows are triggered by `v*` tags.

```sh
git tag v0.1.1
git push origin v0.1.1
```

## 2. GitHub Actions outputs

After the tag push:

- `.github/workflows/release-bundles.yml` builds release bundles and uploads them to the GitHub Release.
- `.github/workflows/release-container.yml` builds and publishes `ghcr.io/keli/kelix`.

Confirm both workflows complete successfully for the tag.

## 3. Homebrew tap automation

`release-bundles.yml` includes `update-homebrew-tap`, which:

- runs only on `v*` tags
- requires `HOMEBREW_TAP_TOKEN` secret in `keli/kelix`
- creates a PR in `keli/homebrew-kelix` updating `Formula/kelix.rb`

Required secret:

- `HOMEBREW_TAP_TOKEN`: token with write access to `keli/homebrew-kelix`

If the automation job is skipped or fails, update `keli/homebrew-kelix/Formula/kelix.rb` manually and open a PR.
