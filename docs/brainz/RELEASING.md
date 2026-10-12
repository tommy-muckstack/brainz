# Releasing Brainz

`BRAINZ_VERSION` is the canonical app version. The upstream Cargo package version
stays independent. Describe user-visible changes under `Unreleased` in
`CHANGELOG.md`; never include private notes or credentials in release notes.

## Prepare

1. Run `script/brainz-ci-test`. Review the native UI changes in the real app without
   switching the user's workspace or sending test messages to their assistants.
2. Commit changes, then run `script/brainz-version bump patch` (or `minor`, or
   `preview`). This creates a release commit and moves the Unreleased notes.
3. Review the resulting changelog and version. Push the reviewed source and tag
   `vVERSION`. The release workflow rejects a mismatching tag.

## Build and publish

`script/brainz-release` and the GitHub release workflow use the same build,
signing, notarization, verification, and publishing steps. `--dry-run` builds a
signed, notarized artifact without changing downloads, GitHub releases, or feeds.
`--skip-notarize` is restricted to local rehearsals with both publishing switches
disabled. An existing release output directory or GitHub release is preserved.

Local credentials stay in the ignored `script/brainz-local.env` or the Keychain.
CI uses a `release` environment with an owner approval rule and these secrets:

- `MACOS_CERT_P12_BASE64` and `MACOS_CERT_PASSWORD`
- `APPLE_NOTARY_KEY_P8`, `APPLE_NOTARY_KEY_ID`, `APPLE_NOTARY_ISSUER_ID`
- `BLOB_READ_WRITE_TOKEN`

Configure the environment and secrets in GitHub before enabling tag releases.
The workflow creates a temporary signing keychain and removes its credentials
when finished. It does not read personal OpenAI or Anthropic API keys.

Clean CI builds bundle DM Sans and can use locally installed Gellix. A local
checkout with Gellix font assets requires `BRAINZ_ALLOW_GELLIX=1` after the owner
confirms redistribution rights. Do not commit commercial font files.

Publishing order is immutable versioned DMG, downloaded-byte verification,
GitHub release, stable download alias when applicable, alias verification, then
the channel feed. Preview releases use `brainz-preview.json`; stable releases
keep `brainz-latest.json` for older clients. The feed includes SHA-256, size,
commit, notes, channel, rollout, and pause state. The client verifies the
compiled-in signing identity independently of feed metadata.

## Smoke checks

Run `script/brainz-smoke APP DMG` to verify signatures, notarization, and bundled
helpers. Before promotion, also check:

- Open the existing brain and a Markdown note; verify its folders remain intact.
- Open a PDF, Calendar, Brief, Themes, and Connectors.
- Open a saved canvas; drag, edit, connect, copy PNG, and cancel a folder move.
- Type in a disposable chat draft, including a slash canvas reference, without
  sending it. Verify existing drafts are preserved.
- Check for updates and open the installed version's release notes.

## Pause and rollback

Set `paused: true` in the affected channel feed to stop both automatic and manual
updates for clients that understand feed v2. Preserve the versioned DMG and notes
for auditability. Older clients ignore pause metadata, so point the feed and
stable alias back to the last verified artifact when necessary. Never overwrite
an immutable versioned artifact.

The updater does not downgrade installed apps. Ship a higher patch version that
reverts the faulty change, or install the previous signed DMG manually after
quitting Brainz. Preserve the user's files and settings. Recheck the public DMG
hash before publishing a replacement feed.

## Validation record

- 0.1.38 app and DMG accepted by the new signature, signing-team, notarization,
  stapling, and helper checks on the release Mac.
- Feed generation, changelog extraction, preview promotion, checksum rejection,
  pause/rollout, and local feature-flag behavior have automated tests.
- A clean CI signing rehearsal remains pending the owner-managed release secrets
  and environment approval rule.
