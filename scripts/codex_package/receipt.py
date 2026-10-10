"""Package-time build receipt: binds a package directory to its source state."""

import hashlib
import json
import os
import subprocess
from datetime import datetime, timezone
from pathlib import Path

RECEIPT_NAME = "codex-build-receipt.json"


def _git(repo_root: Path, *args: str) -> bytes:
    try:
        completed = subprocess.run(
            ["git", "-C", str(repo_root), *args],
            check=True,
            capture_output=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        stderr = getattr(error, "stderr", b"") or b""
        raise RuntimeError(
            f"cannot bind build receipt: git {' '.join(args)} failed in {repo_root}: "
            f"{error} {stderr.decode('utf-8', 'replace').strip()}"
        ) from error
    return completed.stdout


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _git_toplevel(repo_root: Path) -> Path:
    top = Path(_git(repo_root, "rev-parse", "--show-toplevel").decode().strip())
    if os.path.normcase(str(top.resolve())) != os.path.normcase(
        str(repo_root.resolve())
    ):
        raise RuntimeError(
            f"cannot bind build receipt: {repo_root} is not the git top-level "
            f"(resolved {top}); refusing to record a parent repository's state"
        )
    return top


def _dirty_paths(repo_root: Path, exclude: Path | None) -> list[bytes]:
    tracked = _git(
        repo_root, "diff", "HEAD", "--name-only", "-z", "--no-renames", "--no-ext-diff"
    )
    untracked = _git(
        repo_root, "ls-files", "-z", "--others", "--exclude-standard"
    )
    flagged = _git(repo_root, "ls-files", "-v", "-z")
    paths = {p for p in tracked.split(b"\0") if p}
    paths |= {p for p in untracked.split(b"\0") if p}
    # skip-worktree ('S') and assume-unchanged (lowercase) entries are invisible
    # to status/diff; record them so a clean-looking tree cannot hide edits.
    for entry in flagged.split(b"\0"):
        if len(entry) > 2 and (entry[:1] == b"S" or entry[:1].islower()):
            paths.add(entry[2:])
    if exclude is not None:
        prefix = exclude.as_posix().encode("utf-8")
        paths = {p for p in paths if p != prefix and not p.startswith(prefix + b"/")}
    return sorted(paths)


def _source_state(
    repo_root: Path, exclude: Path | None
) -> tuple[bool, str | None, list[str]]:
    paths = _dirty_paths(repo_root, exclude)
    if not paths:
        return False, None, []
    digest = hashlib.sha256()
    diff = _git(
        repo_root,
        "-c",
        "core.quotepath=off",
        "diff",
        "HEAD",
        "--binary",
        "--full-index",
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
    )
    digest.update(b"diff:%d\0" % len(diff))
    digest.update(diff)
    for raw in paths:
        candidate = repo_root / os.fsdecode(raw)
        if candidate.is_file():
            digest.update(b"file:%d\0" % len(raw))
            digest.update(raw)
            digest.update(_sha256_file(candidate).encode("ascii"))
        else:
            digest.update(b"gone:%d\0" % len(raw))
            digest.update(raw)
    return True, digest.hexdigest(), [os.fsdecode(p) for p in paths]


def write_build_receipt(package_dir: Path, *, repo_root: Path, version: str) -> dict:
    repo_root = _git_toplevel(repo_root)
    try:
        exclude = package_dir.resolve().relative_to(repo_root.resolve())
    except ValueError:
        exclude = None
    head_before = _git(repo_root, "rev-parse", "HEAD").decode().strip()
    dirty, diff_sha256, dirty_paths = _source_state(repo_root, exclude)
    head_after = _git(repo_root, "rev-parse", "HEAD").decode().strip()
    if head_before != head_after:
        raise RuntimeError("source HEAD moved while writing build receipt")
    files = {
        path.relative_to(package_dir).as_posix(): _sha256_file(path)
        for path in sorted(package_dir.rglob("*"))
        if path.is_file() and path.relative_to(package_dir).as_posix() != RECEIPT_NAME
    }
    body = {
        "receiptVersion": 1,
        "stage": "assembled",
        "version": version,
        "createdUtc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "sourceRevision": head_before,
        "sourceBranch": _git(repo_root, "rev-parse", "--abbrev-ref", "HEAD")
        .decode()
        .strip(),
        "sourceDirty": dirty,
        "sourceDiffSha256": diff_sha256,
        "sourceDirtyPaths": dirty_paths,
        "files": files,
    }
    canonical = json.dumps(body, sort_keys=True, separators=(",", ":")).encode()
    receipt = {**body, "receiptSha256": hashlib.sha256(canonical).hexdigest()}
    with open(package_dir / RECEIPT_NAME, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(receipt, fh, indent=2, sort_keys=True)
        fh.write("\n")
    return receipt
