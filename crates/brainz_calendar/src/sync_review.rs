use std::{collections::HashSet, path::Path};

use anyhow::{Context as _, Result, bail, ensure};

use crate::github_sync::{command, run};

const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
const SECRET_MARKERS: &[(&[u8], &str)] = &[
    (b"-----BEGIN ", "a private key"),
    (b"sk-ant-", "an Anthropic API key"),
    (b"sk-proj-", "an OpenAI API key"),
    (b"ghp_", "a GitHub token"),
    (b"github_pat_", "a GitHub token"),
    (b"xoxb-", "a Slack bot token"),
    (b"xoxp-", "a Slack user token"),
    (b"phx_", "a PostHog personal API key"),
];

fn contains_aws_access_key(content: &[u8]) -> bool {
    content.windows(20).any(|candidate| {
        (candidate.starts_with(b"AKIA") || candidate.starts_with(b"ASIA"))
            && candidate[4..]
                .iter()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    })
}

#[allow(clippy::disallowed_methods)]
fn git_bytes(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = command(repo, "git").args(args).output()?;
    ensure!(
        output.status.success(),
        "Git could not read the content for sync review."
    );
    Ok(output.stdout)
}

fn paths(repo: &Path, args: &[&str]) -> Result<Vec<String>> {
    let output = String::from_utf8(git_bytes(repo, args)?)
        .context("A filename is not valid UTF-8; sync stopped before publishing it")?;
    Ok(output
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect())
}

fn review_paths(
    repo: &Path,
    tree: &str,
    changed: &[String],
    reviewed_blobs: &mut HashSet<String>,
) -> Result<()> {
    let changed: HashSet<&str> = changed.iter().map(String::as_str).collect();
    let listing = String::from_utf8(git_bytes(repo, &["ls-tree", "-r", "-l", "-z", tree])?)
        .context("A filename is not valid UTF-8; sync stopped before publishing it")?;
    let mut found = HashSet::new();
    for entry in listing.split('\0').filter(|entry| !entry.is_empty()) {
        let (header, path) = entry.split_once('\t').context("Invalid Git tree entry")?;
        if !changed.contains(path) {
            continue;
        }
        found.insert(path);
        let mut fields = header.split_ascii_whitespace();
        let _mode = fields.next().context("Missing Git entry mode")?;
        let kind = fields.next().context("Missing Git entry kind")?;
        let object = fields.next().context("Missing Git object")?;
        ensure!(
            kind == "blob",
            "{path} is not a file that Brainz can review. Sync stopped."
        );
        let size: u64 = fields.next().context("Missing file size")?.parse()?;
        let name = Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .context("Missing filename")?
            .to_lowercase();
        ensure!(
            !(name == ".env"
                || name.starts_with(".env.")
                || name.ends_with(".pem")
                || name.ends_with(".p12")
                || name == "id_rsa"
                || name == "id_ed25519"),
            "Review stopped the sync: {path} looks like a credentials file."
        );
        ensure!(
            size <= MAX_FILE_BYTES,
            "Review stopped the sync: {path} exceeds 25 MB."
        );
        if reviewed_blobs.insert(object.to_owned()) {
            let content = git_bytes(repo, &["cat-file", "blob", object])?;
            if contains_aws_access_key(&content) {
                bail!("Review stopped the sync: {path} appears to contain an AWS access key.");
            }
            for (marker, description) in SECRET_MARKERS {
                if content
                    .windows(marker.len())
                    .any(|window| window == *marker)
                {
                    bail!("Review stopped the sync: {path} appears to contain {description}.");
                }
            }
        }
    }
    ensure!(
        found.len() == changed.len(),
        "Some files could not be reviewed; sync stopped."
    );
    Ok(())
}

pub(crate) struct ReviewedTree {
    pub tree: String,
    pub paths: Vec<String>,
    pub head: String,
    pub branch: String,
}

pub(crate) fn staged_tree(repo: &Path) -> Result<ReviewedTree> {
    let head = run(repo, "git", &["rev-parse", "HEAD"])?;
    let branch = run(repo, "git", &["symbolic-ref", "HEAD"])?;
    let tree = run(repo, "git", &["write-tree"])?;
    let paths = paths(
        repo,
        &[
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            &head,
            &tree,
            "--",
        ],
    )?;
    let contents = paths_for_diff(repo, &head, &tree)?;
    review_paths(repo, &tree, &contents, &mut HashSet::new())?;
    Ok(ReviewedTree {
        tree,
        paths,
        head,
        branch,
    })
}

fn paths_for_diff(repo: &Path, before: &str, after: &str) -> Result<Vec<String>> {
    paths(
        repo,
        &[
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            "--diff-filter=ACMRT",
            before,
            after,
            "--",
        ],
    )
}

pub(crate) fn outgoing_commits(repo: &Path, base: &str, head: &str) -> Result<usize> {
    let range = format!("{base}..{head}");
    let commits = run(repo, "git", &["rev-list", "--reverse", &range])?;
    let mut reviewed_blobs = HashSet::new();
    let mut count = 0;
    for commit in commits.lines().filter(|commit| !commit.is_empty()) {
        // A merge that folded `base` in is reviewed only for what it adds
        // beyond the published side; its other parent's content is already
        // on GitHub and would otherwise be re-scanned on every later sync.
        let published_parent = published_parent(repo, base, commit)?;
        let mut args = vec![
            "diff-tree",
            "--root",
            "--no-commit-id",
            "-r",
            "--name-only",
            "-z",
            "--no-renames",
            "--diff-filter=ACMRT",
        ];
        match published_parent.as_deref() {
            Some(parent) => args.extend([parent, commit]),
            None => args.extend(["-m", commit]),
        }
        let changed = paths(repo, &args)?;
        review_paths(repo, commit, &changed, &mut reviewed_blobs)?;
        count += 1;
    }
    Ok(count)
}

/// For a merge commit, the parent already contained in `base`, if any.
fn published_parent(repo: &Path, base: &str, commit: &str) -> Result<Option<String>> {
    let parents = run(repo, "git", &["rev-list", "--parents", "-n", "1", commit])?;
    let parents = parents.split_whitespace().skip(1).collect::<Vec<_>>();
    if parents.len() < 2 {
        return Ok(None);
    }
    Ok(parents
        .into_iter()
        .find(|parent| run(repo, "git", &["merge-base", "--is-ancestor", parent, base]).is_ok())
        .map(str::to_owned))
}

#[allow(clippy::disallowed_methods)]
pub(crate) fn commit(repo: &Path, reviewed: &ReviewedTree, message: &str) -> Result<()> {
    // A private index keeps later staging by another process out of this
    // commit, without replacing that process's work in the real index.
    let directory = tempfile::tempdir()?;
    let index = directory.path().join("index");
    for args in [
        vec!["read-tree", reviewed.tree.as_str()],
        vec!["commit", "-m", message],
    ] {
        ensure!(
            run(repo, "git", &["rev-parse", "HEAD"])? == reviewed.head
                && run(repo, "git", &["symbolic-ref", "HEAD"])? == reviewed.branch,
            "The branch or its commits changed during review. Local work was kept; sync again to review it."
        );
        let output = command(repo, "git")
            .env("GIT_INDEX_FILE", &index)
            .args(args)
            .output()?;
        if !output.status.success() {
            bail!(
                "Could not commit reviewed changes: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    ensure!(
        run(repo, "git", &["rev-parse", "HEAD^{tree}"])? == reviewed.tree,
        "A Git hook changed the reviewed commit. It was kept locally; sync stopped before publishing."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aws_review_requires_a_complete_access_key_id() {
        assert!(!contains_aws_access_key(b"AKIAa1-not-a-key"));
        assert!(contains_aws_access_key(
            b"prefix AKIAIOSFODNN7EXAMPLE suffix"
        ));
        assert!(contains_aws_access_key(b"ASIAIOSFODNN7EXAMPLE"));
    }

    struct Repository {
        directory: tempfile::TempDir,
        base: String,
    }

    impl Repository {
        fn new() -> Self {
            let directory = tempfile::tempdir().expect("fixture directory");
            let repo = directory.path();
            run(repo, "git", &["init", "--initial-branch=main"]).expect("init");
            run(repo, "git", &["config", "user.name", "Review Fixture"]).expect("author");
            run(
                repo,
                "git",
                &["config", "user.email", "fixture@example.invalid"],
            )
            .expect("email");
            run(repo, "git", &["config", "commit.gpgsign", "false"]).expect("unsigned fixture");
            std::fs::write(repo.join("note.md"), "Original note\n").expect("note");
            run(repo, "git", &["add", "note.md"]).expect("stage");
            run(repo, "git", &["commit", "-m", "Initial fixture"]).expect("commit");
            let base = run(repo, "git", &["rev-parse", "HEAD"]).expect("base");
            Self { directory, base }
        }

        fn path(&self) -> &Path {
            self.directory.path()
        }

        fn stage_all(&self) {
            run(self.path(), "git", &["add", "-A"]).expect("stage fixture");
        }
    }

    #[test]
    fn sync_review_checks_nested_names_and_binary_or_large_content() {
        for name in [
            "nested folder/ a \"quoted\"\nname.md",
            "nested folder/.env",
            "nested folder/data.bin",
        ] {
            let repository = Repository::new();
            std::fs::create_dir_all(repository.path().join("nested folder")).expect("folder");
            let mut content = vec![b'a'; 2 * 1024 * 1024 + 12];
            content.extend_from_slice(b"sk-ant-fixture-not-a-key");
            content.push(0xff);
            std::fs::write(repository.path().join(name), &content).expect("fixture content");
            repository.stage_all();
            let error = staged_tree(repository.path())
                .err()
                .expect("review rejects content");
            assert!(error.to_string().contains(name));
            assert!(!error.to_string().contains("fixture-not-a-key"));
            assert_eq!(
                run(repository.path(), "git", &["rev-parse", "HEAD"]).expect("head"),
                repository.base
            );
            assert_eq!(
                std::fs::read(repository.path().join(name)).expect("unchanged file"),
                content
            );
        }
    }

    #[test]
    fn sync_review_reads_the_staged_blob_not_the_working_file() {
        let repository = Repository::new();
        let path = repository.path().join("note.md");
        std::fs::write(&path, "Safe staged note\n").expect("safe text");
        repository.stage_all();
        std::fs::write(&path, "sk-proj-fixture-not-a-key\n").expect("uncommitted text");
        let reviewed = staged_tree(repository.path()).expect("only staged content is reviewed");
        repository.stage_all();
        std::fs::write(&path, "Safe working text\n").expect("working text");
        assert!(staged_tree(repository.path()).is_err());
        commit(repository.path(), &reviewed, "Reviewed snapshot")
            .expect("commit exact reviewed tree");
        assert_eq!(
            run(repository.path(), "git", &["show", "HEAD:note.md"]).expect("commit text"),
            "Safe staged note"
        );
        assert_eq!(
            run(repository.path(), "git", &["show", ":note.md"]).expect("staged text"),
            "sk-proj-fixture-not-a-key"
        );
        assert_eq!(
            std::fs::read_to_string(path).expect("working text"),
            "Safe working text\n"
        );
    }

    #[test]
    fn sync_review_rejects_secrets_removed_in_later_unpublished_commits() {
        let repository = Repository::new();
        std::fs::write(
            repository.path().join("temporary.md"),
            "ghp_fixture-not-a-token",
        )
        .expect("fixture");
        repository.stage_all();
        run(
            repository.path(),
            "git",
            &["commit", "-m", "Unsafe intermediate fixture"],
        )
        .expect("fixture commit");
        std::fs::remove_file(repository.path().join("temporary.md")).expect("remove fixture");
        repository.stage_all();
        run(
            repository.path(),
            "git",
            &["commit", "-m", "Remove fixture"],
        )
        .expect("fixture commit");
        let head = run(repository.path(), "git", &["rev-parse", "HEAD"]).expect("head");
        assert!(outgoing_commits(repository.path(), &repository.base, &head).is_err());
        assert_eq!(
            run(repository.path(), "git", &["rev-parse", "HEAD"]).expect("unchanged head"),
            head
        );
    }

    #[test]
    fn sync_review_handles_renames_deletions_and_safe_history() {
        let repository = Repository::new();
        std::fs::rename(
            repository.path().join("note.md"),
            repository.path().join(" renamed\n😀.md"),
        )
        .expect("rename");
        repository.stage_all();
        let reviewed = staged_tree(repository.path()).expect("review rename");
        assert_eq!(reviewed.paths.len(), 2);
        commit(repository.path(), &reviewed, "Rename fixture").expect("commit rename");
        std::fs::write(repository.path().join("second.md"), "Second note\r\n").expect("new note");
        repository.stage_all();
        commit(
            repository.path(),
            &staged_tree(repository.path()).expect("review"),
            "Add fixture",
        )
        .expect("commit");
        let head = run(repository.path(), "git", &["rev-parse", "HEAD"]).expect("head");
        assert_eq!(
            outgoing_commits(repository.path(), &repository.base, &head).expect("safe history"),
            2
        );
        assert!(
            run(repository.path(), "git", &["status", "--porcelain"])
                .expect("status")
                .is_empty()
        );
    }

    #[test]
    fn sync_review_rechecks_filenames_for_previously_reviewed_blobs() {
        let repository = Repository::new();
        std::fs::write(repository.path().join("example.md"), "Same content\n").expect("note");
        repository.stage_all();
        run(repository.path(), "git", &["commit", "-m", "Safe name"]).expect("fixture commit");
        std::fs::rename(
            repository.path().join("example.md"),
            repository.path().join(".env"),
        )
        .expect("rename");
        repository.stage_all();
        run(
            repository.path(),
            "git",
            &["commit", "-m", "Unsafe filename fixture"],
        )
        .expect("fixture commit");
        let head = run(repository.path(), "git", &["rev-parse", "HEAD"]).expect("head");
        assert!(
            outgoing_commits(repository.path(), &repository.base, &head)
                .expect_err("review path too")
                .to_string()
                .contains("credentials")
        );
    }

    #[test]
    fn sync_review_stops_if_the_branch_changes_after_review() {
        let repository = Repository::new();
        std::fs::write(repository.path().join("note.md"), "Edited note\n").expect("note");
        repository.stage_all();
        let reviewed = staged_tree(repository.path()).expect("review");
        run(repository.path(), "git", &["switch", "-c", "other-work"]).expect("switch branch");
        assert!(commit(repository.path(), &reviewed, "Must not commit").is_err());
        assert_eq!(
            run(repository.path(), "git", &["rev-parse", "HEAD"]).expect("head"),
            repository.base
        );
        assert_eq!(
            std::fs::read_to_string(repository.path().join("note.md")).expect("note"),
            "Edited note\n"
        );
    }
}
