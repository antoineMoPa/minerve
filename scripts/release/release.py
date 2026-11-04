#!/usr/bin/env python3
"""Build and ship only explicitly listed native release files. Python 3.11+."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import time
import tomllib

ROOT = Path(__file__).resolve().parents[2]
REPO = "antoineMoPa/minerve"
TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
)


def run(*args, capture=False, env=None):
    result = subprocess.run(args, cwd=ROOT, check=True, text=True,
                            stdout=subprocess.PIPE if capture else None, env=env)
    return result.stdout.strip() if capture else ""


def version():
    return tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]


def next_release_version():
    current = version()
    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", current)
    if match is None:
        raise ValueError("Cargo.toml package version must be MAJOR.MINOR.PATCH")
    major, minor, _patch = map(int, match.groups())
    return f"{major}.{minor + 1}.0"


def check_tag(tag):
    if not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
        raise ValueError("release tag must be vMAJOR.MINOR.PATCH")
    if tag != f"v{version()}":
        raise ValueError("tag must match Cargo.toml's package version")


def host_target():
    targets = {
        ("Darwin", "arm64"): "aarch64-apple-darwin",
        ("Linux", "x86_64"): "x86_64-unknown-linux-gnu",
        ("Linux", "aarch64"): "aarch64-unknown-linux-gnu",
    }
    try:
        return targets[(platform.system(), platform.machine())]
    except KeyError:
        raise ValueError(f"unsupported build host: {platform.system()} {platform.machine()}") from None


def build_env():
    # Deliberate allowlist: do not pass personal tokens, application settings, .env,
    # RUSTFLAGS, or arbitrary build-time variables into the public executable.
    keys = ("PATH", "HOME", "TMPDIR", "RUSTUP_HOME", "CARGO_HOME")
    return {key: os.environ[key] for key in keys if key in os.environ}


def archive_name(target):
    return f"minerve-{target}.tar.gz"


def package(tag, target):
    check_tag(tag)
    binary = ROOT / "target" / target / "release" / "minerve"
    if not binary.is_file():
        raise ValueError(f"missing executable: {binary}")
    output = ROOT / "target" / "release-artifacts" / tag
    output.mkdir(parents=True, exist_ok=True)
    archive = output / archive_name(target)
    # Explicit file list; no source directory, build cache, credentials or .env.
    files = ((binary, "minerve"), (ROOT / "LICENSE", "LICENSE"))
    with tarfile.open(archive, "w:gz") as bundle:
        for source, name in files:
            info = bundle.gettarinfo(str(source), arcname=name)
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            info.mode = 0o755 if name == "minerve" else 0o644
            with source.open("rb") as stream:
                bundle.addfile(info, stream)
    with archive.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(f"{digest}  {archive.name}\n")
    print(archive)
    return archive


def build(tag, target):
    check_tag(tag)
    if target != host_target():
        raise ValueError("build on the target's native runner")
    run("cargo", "build", "--release", "--locked", "--bin", "minerve",
        "--target", target, "--target-dir", str(ROOT / "target"), env=build_env())
    run(str(ROOT / "target" / target / "release" / "minerve"), "--help", env=build_env())
    return package(tag, target)


def verify(directory):
    for target in TARGETS:
        name = archive_name(target)
        archive = directory / name
        checksum = (directory / (name + ".sha256")).read_text().split()
        with archive.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        if checksum != [digest, name]:
            raise ValueError(f"checksum mismatch: {name}")
        expected = {"minerve", "LICENSE"}
        with tarfile.open(archive) as bundle:
            members = bundle.getnames()
            regular = all(member.isfile() for member in bundle.getmembers())
        if len(members) != 2 or set(members) != expected or not regular:
            raise ValueError(f"unexpected archive contents: {name}")
    print("All three platform archives and checksums verified.")


def gh(*args, capture=False):
    return run("gh", *args, "--repo", REPO, capture=capture)


def ensure_workflow_on_default_branch():
    # GitHub only accepts workflow_dispatch for a workflow file on the default branch.
    try:
        gh("workflow", "view", "native-release.yml", capture=True)
    except subprocess.CalledProcessError:
        raise ValueError("native-release.yml is not on the repository's default branch; push it first") from None


def release_draft_state(tag):
    """Return True for a draft release, False for a published one, None if no release exists."""
    releases = json.loads(gh("release", "list", "--json", "tagName,isDraft", "--limit", "200", capture=True))
    for release in releases:
        if release["tagName"] == tag:
            return release["isDraft"]
    return None


def create_draft(tag, new_version):
    gh("release", "create", tag, "--verify-tag", "--draft", "--title", tag,
       "--notes", f"Minerve {new_version}: native macOS Apple Silicon, Linux x64/ARM64 builds.")


def publish(resume):
    target = host_target()
    if run("git", "status", "--porcelain", capture=True):
        raise ValueError("commit your work first; publishing requires a clean worktree")
    run("gh", "auth", "status")
    ensure_workflow_on_default_branch()
    if resume:
        new_version = version()
        tag = f"v{new_version}"
        check_tag(tag)
        if run("git", "rev-parse", f"{tag}^{{commit}}", capture=True) != run("git", "rev-parse", "HEAD", capture=True):
            raise ValueError("resume from the release tag's exact commit")
        if not run("git", "ls-remote", "--tags", "origin", f"refs/tags/{tag}", capture=True):
            raise ValueError(f"tag {tag} is not pushed; run: git push origin {tag}")
        is_draft = release_draft_state(tag)
        if is_draft is None:
            # The tag was pushed but the draft was never created (e.g. gh failed).
            create_draft(tag, new_version)
        elif not is_draft:
            raise ValueError("release is already published; resume only works on a draft")
    else:
        new_version = next_release_version()
        tag = f"v{new_version}"
        run("git", "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}", capture=True)
        tags = run("git", "ls-remote", "--tags", "origin", f"refs/tags/{tag}", capture=True)
        if tags or run("git", "tag", "--list", tag, capture=True):
            raise ValueError(f"tag {tag} already exists")
        run("git", "push", "--dry-run", "origin", "HEAD")
        manifest = ROOT / "Cargo.toml"
        text = manifest.read_text()
        start = text.index("[package]\n")
        end = text.index("\n[", start + 1)
        block, count = re.subn(r'^version\s*=\s*"[^\"]+"', f'version = "{new_version}"', text[start:end], count=1, flags=re.M)
        if count != 1:
            raise ValueError("could not update package version")
        try:
            manifest.write_text(text[:start] + block + text[end:])
            run("cargo", "update", "--workspace", "--offline", env=build_env())
            # Finish the local build before creating any commit/tag/release.
            build(tag, target)
        except BaseException:
            # The worktree was verified clean above, so restoring these two files loses nothing.
            run("git", "checkout", "--", "Cargo.toml", "Cargo.lock")
            raise
        run("git", "add", "Cargo.toml", "Cargo.lock")
        run("git", "commit", "-m", f"chore(release): {tag}")
        run("git", "tag", "-a", tag, "-m", tag)
        run("git", "push", "--atomic", "origin", "HEAD", f"refs/tags/{tag}")
        create_draft(tag, new_version)
    if resume:
        build(tag, target)
    archive = ROOT / "target" / "release-artifacts" / tag / archive_name(target)
    gh("release", "upload", tag, str(archive), str(archive) + ".sha256", "--clobber")
    # The installer is a versioned release asset, too.
    gh("release", "upload", tag, str(ROOT / "install.sh"), "--clobber")
    previous_runs = json.loads(gh("run", "list", "--workflow", "native-release.yml", "--branch", tag,
                                  "--event", "workflow_dispatch", "--json", "databaseId", "--limit", "20", capture=True))
    previous_ids = {job["databaseId"] for job in previous_runs}
    gh("workflow", "run", "native-release.yml", "--ref", tag, "-f", f"local_target={target}")
    run_id = None
    for _ in range(60):
        runs = json.loads(gh("run", "list", "--workflow", "native-release.yml", "--branch", tag,
                             "--event", "workflow_dispatch", "--json", "databaseId", "--limit", "20", capture=True))
        for job in runs:
            if job["databaseId"] not in previous_ids:
                run_id = str(job["databaseId"])
                break
        if run_id:
            break
        time.sleep(2)
    if not run_id:
        raise ValueError("workflow did not appear; the draft release stays unpublished")
    gh("run", "watch", run_id, "--exit-status")
    # Download into an empty directory so stale local files cannot mask missing uploads.
    output = ROOT / "target" / "release-artifacts" / tag / "downloaded"
    if output.exists():
        shutil.rmtree(output)
    gh("release", "download", tag, "--dir", str(output))
    verify(output)
    if (output / "install.sh").read_bytes() != (ROOT / "install.sh").read_bytes():
        raise ValueError("uploaded installer differs: install.sh")
    gh("release", "edit", tag, "--draft=false", "--latest")
    print(f"Published https://github.com/{REPO}/releases/tag/{tag}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for command in ("build", "package"):
        p = sub.add_parser(command)
        p.add_argument("tag")
        p.add_argument("target", choices=TARGETS)
    p = sub.add_parser("verify")
    p.add_argument("directory", type=Path)
    p = sub.add_parser("publish")
    p.add_argument("--resume", action="store_true")
    args = parser.parse_args()
    if args.command == "build":
        build(args.tag, args.target)
    elif args.command == "package":
        package(args.tag, args.target)
    elif args.command == "verify":
        verify(args.directory)
    else:
        publish(args.resume)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(f"Release stopped: {error}\nNothing is published. If the tag was already pushed, fix the cause and run scripts/publishFlow.sh --resume from the tag's commit.")
