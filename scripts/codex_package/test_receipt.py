#!/usr/bin/env python3

from pathlib import Path
import hashlib
import json
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from codex_package.receipt import RECEIPT_NAME
from codex_package.receipt import write_build_receipt


def git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def make_repo(root: Path) -> Path:
    repo = root / "repo"
    repo.mkdir()
    git(repo, "init", "-q")
    git(repo, "config", "user.email", "t@example.com")
    git(repo, "config", "user.name", "t")
    (repo / "a.txt").write_text("one\n")
    git(repo, "add", "a.txt")
    git(repo, "commit", "-q", "-m", "init")
    return repo


def make_package(root: Path) -> Path:
    package = root / "pkg"
    (package / "bin").mkdir(parents=True)
    (package / "bin" / "codex.exe").write_bytes(b"cli")
    (package / "codex-package.json").write_text("{}")
    return package


class BuildReceiptTest(unittest.TestCase):
    def test_clean_tree_records_head_and_file_hashes(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            repo = make_repo(root)
            package = make_package(root)
            receipt = write_build_receipt(package, repo_root=repo, version="0.160.0")
            on_disk = json.loads((package / RECEIPT_NAME).read_text())
            self.assertEqual(receipt, on_disk)
            self.assertEqual(on_disk["sourceRevision"], git(repo, "rev-parse", "HEAD"))
            self.assertFalse(on_disk["sourceDirty"])
            self.assertIsNone(on_disk["sourceDiffSha256"])
            self.assertEqual(on_disk["version"], "0.160.0")
            self.assertEqual(
                on_disk["files"]["bin/codex.exe"], hashlib.sha256(b"cli").hexdigest()
            )
            self.assertIn("codex-package.json", on_disk["files"])
            self.assertNotIn(RECEIPT_NAME, on_disk["files"])

    def test_dirty_tree_records_diff_digest_including_untracked(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            repo = make_repo(root)
            package = make_package(root)
            (repo / "a.txt").write_text("two\n")
            (repo / "new.txt").write_text("untracked\n")
            receipt = write_build_receipt(package, repo_root=repo, version="0.160.0")
            self.assertTrue(receipt["sourceDirty"])
            self.assertRegex(receipt["sourceDiffSha256"], r"^[0-9a-f]{64}$")
            self.assertIn("a.txt", receipt["sourceDirtyPaths"])
            self.assertIn("new.txt", receipt["sourceDirtyPaths"])

    def test_diff_digest_changes_when_source_changes(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            repo = make_repo(root)
            package = make_package(root)
            (repo / "a.txt").write_text("two\n")
            first = write_build_receipt(package, repo_root=repo, version="0.160.0")
            (repo / "a.txt").write_text("three\n")
            second = write_build_receipt(package, repo_root=repo, version="0.160.0")
            self.assertNotEqual(first["sourceDiffSha256"], second["sourceDiffSha256"])

    def test_rename_is_recorded_with_both_paths_intact(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            repo = make_repo(root)
            package = make_package(root)
            git(repo, "mv", "a.txt", "b b.txt")
            receipt = write_build_receipt(package, repo_root=repo, version="0.160.0")
            self.assertEqual(receipt["sourceDirtyPaths"], ["a.txt", "b b.txt"])

    def test_subdirectory_repo_root_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            repo = make_repo(root)
            package = make_package(root)
            sub = repo / "sub"
            sub.mkdir()
            with self.assertRaises(RuntimeError):
                write_build_receipt(package, repo_root=sub, version="0.160.0")

    def test_non_git_directory_fails_with_clear_error(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            package = make_package(root)
            plain = root / "plain"
            plain.mkdir()
            with self.assertRaises(RuntimeError):
                write_build_receipt(package, repo_root=plain, version="0.160.0")

    def test_untracked_content_change_and_stability(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            repo = make_repo(root)
            package = make_package(root)
            (repo / "new.txt").write_text("x\n")
            first = write_build_receipt(package, repo_root=repo, version="0.160.0")
            again = write_build_receipt(package, repo_root=repo, version="0.160.0")
            self.assertEqual(first["sourceDiffSha256"], again["sourceDiffSha256"])
            (repo / "new.txt").write_text("y\n")
            third = write_build_receipt(package, repo_root=repo, version="0.160.0")
            self.assertNotEqual(first["sourceDiffSha256"], third["sourceDiffSha256"])

    def test_receipt_hash_covers_body_excluding_hash_line(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            repo = make_repo(root)
            package = make_package(root)
            receipt = write_build_receipt(package, repo_root=repo, version="0.160.0")
            body = {k: v for k, v in receipt.items() if k != "receiptSha256"}
            expected = hashlib.sha256(
                json.dumps(body, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest()
            self.assertEqual(receipt["receiptSha256"], expected)


if __name__ == "__main__":
    unittest.main()
