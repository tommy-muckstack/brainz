# Brainz automation

Only `brainz-*.yml` workflows are intended to run in this repository. The other
workflow files are retained for upstream merge compatibility.

`brainz-ci.yml` checks formatting, keymaps, workflows, native code, icons, and
the whiteboard. `script/brainz-ci-test` runs the same checks locally. Install
actionlint 1.7.12 and the Playwright Chromium browser first. On a Mac with Chrome
already installed, `BRAINZ_TEST_BROWSER` can point to its executable.

The native crates depend on GPUI, including `brainz_ai`, `cli`, and
`release_channel`, so they run on macOS. Dependency-free model crates and icon
checks run on Linux. Both jobs are required; the Linux job is not a replacement
for native UI tests.

Repository administration remains a separate owner action. After this workflow
lands, disable the inherited workflows and protect `main` with required checks
`lint`, `test-linux`, `test-macos`, and `canvas`. Use strict status checks with
administrator bypass available for documented emergency fixes.

To inspect active workflows:

```sh
gh workflow list --all --repo tommy-muckstack/brainz
```

To disable one inherited workflow (repeat for every file without a `brainz-`
prefix):

```sh
gh workflow disable run_tests.yml --repo tommy-muckstack/brainz
```

The owner can apply branch protection with this JSON request:

```sh
gh api --method PUT repos/tommy-muckstack/brainz/branches/main/protection --input - <<'JSON'
{
  "required_status_checks": {
    "strict": true,
    "contexts": ["lint", "test-linux", "test-macos", "canvas"]
  },
  "enforce_admins": false,
  "required_pull_request_reviews": null,
  "restrictions": null
}
JSON
```
