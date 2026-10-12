import hashlib
from pathlib import Path
import tempfile
import unittest
from brainz_release import bump_changelog, feed, next_version, release_notes, validate_version


class ReleaseTests(unittest.TestCase):
    def test_versions_and_preview_promotion(self):
        self.assertEqual(next_version("0.1.38", "patch"), "0.1.39")
        self.assertEqual(next_version("0.1.38", "minor"), "0.2.0")
        self.assertEqual(next_version("0.1.38", "preview"), "0.1.39-preview.1")
        self.assertEqual(next_version("0.1.39-preview.1", "preview"), "0.1.39-preview.2")
        self.assertEqual(next_version("0.1.39-preview.2", "patch"), "0.1.39")
        for invalid in ["1", "v0.1.0", "0.01.0", "0.1.0-preview.0", "../../etc", "0.1.0;exit"]:
            with self.assertRaises(ValueError):
                validate_version(invalid)

    def test_notes_do_not_include_adjacent_releases(self):
        changelog = "# Changelog\n\n## Unreleased\n\n- New\n\n## 0.1.38 (2026-10-11)\n\n### Fixed\n\n- Old\n"
        updated = bump_changelog(changelog, "0.1.39", "2026-10-12")
        self.assertEqual(release_notes(updated, "0.1.39"), "- New\n")
        self.assertEqual(release_notes(updated, "0.1.38"), "### Fixed\n\n- Old\n")
        with self.assertRaises(ValueError):
            release_notes(updated, "0.2.0")
        with self.assertRaises(ValueError):
            bump_changelog(updated, "0.1.39", "2026-10-12")

    def test_feed_hashes_the_final_artifact_and_keeps_legacy_fields(self):
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "brainz.dmg"
            artifact.write_bytes(b"signed and stapled disk image")
            result = feed(artifact, "0.1.39", "https://example.com/brainz.dmg", "a" * 40, "Notes\n", 25, True)
            self.assertEqual(result["sha256"], hashlib.sha256(artifact.read_bytes()).hexdigest())
            self.assertEqual(result["size"], artifact.stat().st_size)
            self.assertEqual(result["version"], "0.1.39")
            self.assertEqual(result["url"], "https://example.com/brainz.dmg")
            self.assertEqual(result["rollout"], 25)
            self.assertTrue(result["paused"])
            for url in ["http://example.com/app.dmg", "file:///app.dmg", "https://secret@example.com/app.dmg"]:
                with self.assertRaises(ValueError):
                    feed(artifact, "0.1.39", url, "a" * 40, "Notes")


if __name__ == "__main__":
    unittest.main()
