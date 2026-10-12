"""Shared version, changelog, and update-feed rules for Brainz releases."""

import datetime
import hashlib
import json
from pathlib import Path
import re
import subprocess
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parent.parent
VERSION_PATTERN = re.compile(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-preview\.([1-9]\d*))?")


def validate_version(version):
    if not VERSION_PATTERN.fullmatch(version):
        raise ValueError(f"Invalid Brainz version: {version!r}")
    return version


def version(root=ROOT):
    return validate_version((root / "BRAINZ_VERSION").read_text().strip())


def release_notes(changelog, release):
    validate_version(release)
    heading = re.search(rf"^## {re.escape(release)}(?: \(\d{{4}}-\d{{2}}-\d{{2}}\))?\s*$", changelog, re.MULTILINE)
    if not heading:
        raise ValueError(f"CHANGELOG.md has no entry for {release}")
    remainder = changelog[heading.end():]
    boundary = re.search(r"^## ", remainder, re.MULTILINE)
    notes = remainder[:boundary.start()] if boundary else remainder
    if not notes.strip():
        raise ValueError(f"Release notes for {release} are empty")
    return notes.strip() + "\n"


def next_version(current, kind):
    match = VERSION_PATTERN.fullmatch(validate_version(current))
    major, minor, patch = map(int, match.groups()[:3])
    preview = match.group(4)
    if kind == "patch":
        return f"{major}.{minor}.{patch if preview else patch + 1}"
    if kind == "minor":
        return f"{major}.{minor + 1}.0"
    if kind == "preview":
        return f"{major}.{minor}.{patch if preview else patch + 1}-preview.{int(preview) + 1 if preview else 1}"
    raise ValueError(f"Unknown bump: {kind}")


def bump_changelog(changelog, release, date):
    heading = re.search(r"^## Unreleased\s*$", changelog, re.MULTILINE)
    if not heading:
        raise ValueError("CHANGELOG.md needs an Unreleased section")
    if re.search(rf"^## {re.escape(release)}(?:\s|$)", changelog, re.MULTILINE):
        raise ValueError(f"Release {release} already exists in CHANGELOG.md")
    return changelog[:heading.start()] + f"## Unreleased\n\n## {release} ({date})\n" + changelog[heading.end():]


def feed(artifact, release, url, commit, notes, rollout=100, paused=False, date=None):
    validate_version(release)
    parsed = urlparse(url)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password:
        raise ValueError("Release downloads must use HTTPS without credentials")
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("Release commit must be a full Git SHA")
    if not 0 <= rollout <= 100:
        raise ValueError("Rollout must be between 0 and 100")
    digest = hashlib.sha256()
    with artifact.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    if artifact.stat().st_size == 0:
        raise ValueError("The release artifact is empty")
    return {
        "schema": 2, "version": release, "url": url,
        "sha256": digest.hexdigest(), "size": artifact.stat().st_size,
        "commit": commit, "channel": "preview" if "-preview." in release else "stable",
        "notes_markdown": notes, "notes_url": f"https://github.com/tommy-muckstack/brainz/releases/tag/v{release}",
        "released_at": date or datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "rollout": rollout, "paused": paused, "team_id": "K8NAZ76CBQ",
    }


def git(*arguments, root=ROOT):
    return subprocess.check_output(["git", *arguments], cwd=root, text=True).strip()


def write_json(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    temporary.replace(path)
