#!/usr/bin/env python3
"""Validate the contents and normalized manifests of release archives."""

from __future__ import annotations

import argparse
from collections.abc import Iterator, Mapping
from dataclasses import dataclass
import json
from pathlib import Path, PurePosixPath
import re
import sys
import tarfile
import tomllib


@dataclass(frozen=True)
class PackageSpec:
    source_dir: str
    target_kind: str
    target_name: str
    required_files: tuple[str, ...]
    internal_dependencies: Mapping[str, str]


PACKAGES: dict[str, PackageSpec] = {
    "rust-commander-shell": PackageSpec(
        source_dir="crates/shell",
        target_kind="lib",
        target_name="rc_shell",
        required_files=("src/lib.rs",),
        internal_dependencies={},
    ),
    "rust-commander-core": PackageSpec(
        source_dir="crates/core",
        target_kind="lib",
        target_name="rc_core",
        required_files=("src/lib.rs", "assets/mc.default.keymap"),
        internal_dependencies={"rc-shell": "rust-commander-shell"},
    ),
    "rust-commander-ui": PackageSpec(
        source_dir="crates/ui",
        target_kind="lib",
        target_name="rc_ui",
        required_files=("src/lib.rs", "src/skin.rs", "src/bundled_skins.rs"),
        internal_dependencies={"rc-core": "rust-commander-core"},
    ),
    "rust-commander": PackageSpec(
        source_dir="crates/app",
        target_kind="bin",
        target_name="rc",
        required_files=("src/main.rs", "src/runtime.rs"),
        internal_dependencies={
            "rc-core": "rust-commander-core",
            "rc-ui": "rust-commander-ui",
        },
    ),
}

COMMON_FILES = (
    ".cargo_vcs_info.json",
    "Cargo.lock",
    "Cargo.toml",
    "Cargo.toml.orig",
    "LICENSE",
    "README.md",
)


class ReleaseValidationError(ValueError):
    """Raised when a release archive violates the publication contract."""


def read_member(
    archive: tarfile.TarFile, members: Mapping[str, tarfile.TarInfo], name: str
) -> bytes:
    member = members.get(name)
    if member is None:
        raise ReleaseValidationError(f"missing archive member: {name}")
    extracted = archive.extractfile(member)
    if extracted is None:
        raise ReleaseValidationError(f"archive member is not a regular file: {name}")
    return extracted.read()


def dependency_specs(
    manifest: Mapping[str, object],
) -> Iterator[tuple[str, Mapping[str, object]]]:
    table_names = ("dependencies", "dev-dependencies", "build-dependencies")
    containers: list[Mapping[str, object]] = [manifest]

    targets = manifest.get("target", {})
    if isinstance(targets, Mapping):
        containers.extend(
            target for target in targets.values() if isinstance(target, Mapping)
        )

    for container in containers:
        for table_name in table_names:
            table = container.get(table_name, {})
            if not isinstance(table, Mapping):
                continue
            for alias, dependency in table.items():
                if isinstance(alias, str) and isinstance(dependency, Mapping):
                    yield alias, dependency


def validate_manifest(
    manifest: Mapping[str, object],
    *,
    package_name: str,
    version: str,
    spec: PackageSpec,
) -> None:
    package = manifest.get("package")
    if not isinstance(package, Mapping):
        raise ReleaseValidationError(f"{package_name}: missing [package] table")

    expected_metadata: dict[str, object] = {
        "name": package_name,
        "version": version,
        "edition": "2024",
        "rust-version": "1.88.0",
        "license": "GPL-3.0-or-later",
        "repository": "https://github.com/hbbio/rc",
        "readme": "README.md",
        "publish": ["crates-io"],
    }
    for field, expected in expected_metadata.items():
        actual = package.get(field)
        if actual != expected:
            raise ReleaseValidationError(
                f"{package_name}: package.{field} is {actual!r}, expected {expected!r}"
            )
    if not package.get("description"):
        raise ReleaseValidationError(f"{package_name}: package.description is required")

    if spec.target_kind == "lib":
        target = manifest.get("lib")
        if not isinstance(target, Mapping) or target.get("name") != spec.target_name:
            raise ReleaseValidationError(
                f"{package_name}: expected library target {spec.target_name}"
            )
    else:
        targets = manifest.get("bin")
        if not isinstance(targets, list) or len(targets) != 1:
            raise ReleaseValidationError(f"{package_name}: expected exactly one binary")
        target = targets[0]
        if not isinstance(target, Mapping) or target.get("name") != spec.target_name:
            raise ReleaseValidationError(
                f"{package_name}: expected binary target {spec.target_name}"
            )
        if package.get("default-run") != spec.target_name:
            raise ReleaseValidationError(
                f"{package_name}: package.default-run must be {spec.target_name}"
            )

    found_internal: dict[str, str] = {}
    for alias, dependency in dependency_specs(manifest):
        resolved_name = dependency.get("package", alias)
        if not isinstance(resolved_name, str) or not resolved_name.startswith(
            "rust-commander"
        ):
            continue
        if "path" in dependency or "git" in dependency:
            raise ReleaseValidationError(
                f"{package_name}: internal dependency {alias} is not registry-only"
            )
        if dependency.get("version") != version:
            raise ReleaseValidationError(
                f"{package_name}: internal dependency {alias} must require {version}"
            )
        found_internal[alias] = resolved_name

    if found_internal != dict(spec.internal_dependencies):
        raise ReleaseValidationError(
            f"{package_name}: internal dependencies are {found_internal!r}, "
            f"expected {dict(spec.internal_dependencies)!r}"
        )


def validate_member_paths(
    archive: tarfile.TarFile, *, root: str
) -> dict[str, tarfile.TarInfo]:
    members: dict[str, tarfile.TarInfo] = {}
    for member in archive.getmembers():
        path = PurePosixPath(member.name)
        if path.is_absolute() or ".." in path.parts or not path.parts:
            raise ReleaseValidationError(f"unsafe archive path: {member.name}")
        if path.parts[0] != root:
            raise ReleaseValidationError(f"archive member outside {root}: {member.name}")
        if member.issym() or member.islnk():
            raise ReleaseValidationError(f"archive contains a link: {member.name}")
        if member.isfile():
            if member.name in members:
                raise ReleaseValidationError(f"duplicate archive member: {member.name}")
            members[member.name] = member
    return members


def validate_archive(
    repository: Path,
    archives: Path,
    *,
    package_name: str,
    version: str,
    spec: PackageSpec,
    allow_dirty: bool,
) -> str:
    root = f"{package_name}-{version}"
    archive_path = archives / f"{root}.crate"
    if not archive_path.is_file():
        raise ReleaseValidationError(f"missing release archive: {archive_path}")

    with tarfile.open(archive_path, mode="r:gz") as archive:
        members = validate_member_paths(archive, root=root)
        for relative_path in (*COMMON_FILES, *spec.required_files):
            member_name = f"{root}/{relative_path}"
            if member_name not in members:
                raise ReleaseValidationError(
                    f"{package_name}: missing required file {relative_path}"
                )

        license_text = read_member(archive, members, f"{root}/LICENSE")
        if license_text != (repository / "LICENSE").read_bytes():
            raise ReleaseValidationError(f"{package_name}: LICENSE differs from root")

        readme = read_member(archive, members, f"{root}/README.md")
        if not allow_dirty and readme != (repository / "README.md").read_bytes():
            raise ReleaseValidationError(f"{package_name}: README differs from root")

        manifest_text = read_member(archive, members, f"{root}/Cargo.toml")
        manifest = tomllib.loads(manifest_text.decode("utf-8"))
        validate_manifest(
            manifest, package_name=package_name, version=version, spec=spec
        )

        vcs_text = read_member(archive, members, f"{root}/.cargo_vcs_info.json")
        vcs_info = json.loads(vcs_text)
        if not isinstance(vcs_info, Mapping):
            raise ReleaseValidationError(f"{package_name}: invalid VCS provenance")
        git_info = vcs_info.get("git")
        if not isinstance(git_info, Mapping):
            raise ReleaseValidationError(f"{package_name}: missing git provenance")
        revision = git_info.get("sha1")
        if not isinstance(revision, str) or re.fullmatch(r"[0-9a-f]{40}", revision) is None:
            raise ReleaseValidationError(f"{package_name}: invalid git revision")
        if not allow_dirty and git_info.get("dirty", False):
            raise ReleaseValidationError(f"{package_name}: archive has dirty provenance")
        if vcs_info.get("path_in_vcs") != spec.source_dir:
            raise ReleaseValidationError(
                f"{package_name}: incorrect path_in_vcs provenance"
            )

        if package_name == "rust-commander-ui":
            skin_root = repository / "crates/ui/assets/skins"
            expected_skins = {
                f"{root}/assets/skins/{path.relative_to(skin_root).as_posix()}"
                for path in skin_root.rglob("*.ini")
            }
            archived_skins = {
                name for name in members if name.startswith(f"{root}/assets/skins/")
            }
            if archived_skins != expected_skins:
                missing = sorted(expected_skins - archived_skins)
                unexpected = sorted(archived_skins - expected_skins)
                raise ReleaseValidationError(
                    f"{package_name}: skin set differs; "
                    f"missing={missing!r}, unexpected={unexpected!r}"
                )

        print(
            f"validated {archive_path.name}: {len(members)} files, "
            f"{archive_path.stat().st_size} compressed bytes"
        )
        return revision


def workspace_version(repository: Path) -> str:
    with (repository / "Cargo.toml").open("rb") as manifest_file:
        manifest = tomllib.load(manifest_file)
    workspace = manifest.get("workspace")
    if not isinstance(workspace, Mapping):
        raise ReleaseValidationError("root manifest is missing [workspace]")
    package = workspace.get("package")
    if not isinstance(package, Mapping) or not isinstance(package.get("version"), str):
        raise ReleaseValidationError("root manifest is missing workspace.package.version")
    return package["version"]


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True, type=Path)
    parser.add_argument("--archives", required=True, type=Path)
    parser.add_argument("--expected-revision", required=True)
    parser.add_argument("--allow-dirty", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    try:
        repository = args.repository.resolve(strict=True)
        archives = args.archives.resolve(strict=True)
        version = workspace_version(repository)
        revisions = {
            validate_archive(
                repository,
                archives,
                package_name=package_name,
                version=version,
                spec=spec,
                allow_dirty=args.allow_dirty,
            )
            for package_name, spec in PACKAGES.items()
        }
        if revisions != {args.expected_revision}:
            raise ReleaseValidationError(
                f"release archive revisions are {sorted(revisions)!r}, "
                f"expected {args.expected_revision}"
            )
    except (
        OSError,
        json.JSONDecodeError,
        tarfile.TarError,
        tomllib.TOMLDecodeError,
        UnicodeDecodeError,
        ReleaseValidationError,
    ) as error:
        print(f"release package verification failed: {error}", file=sys.stderr)
        return 1

    if args.allow_dirty:
        print(
            f"release package set {version} passed dirty-worktree development checks; "
            "run again from a clean commit before publication"
        )
    else:
        print(f"release package set {version} is self-contained and publication-ready")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
