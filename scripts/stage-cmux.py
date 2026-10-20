#!/usr/bin/env python3
"""Build and stage the local cmux Host V1 package (Python 3.11+)."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib


ROOT = Path(__file__).resolve().parents[1]


def external_directory(value):
    path = Path(value)
    if not path.is_absolute():
        raise ValueError(f"directory must be absolute: {value}")
    path = path.resolve()
    if path == ROOT or ROOT in path.parents:
        raise ValueError(f"directory must be outside the source tree: {path}")
    if any((parent / ".git").exists() for parent in (path, *path.parents)):
        raise ValueError(f"directory must be outside Git checkouts: {path}")
    return path


def stage(output, target_dir):
    # Resolve symlinks before checking containment, including linked checkouts.
    output = external_directory(output)
    target_dir = external_directory(target_dir)
    if output.exists():
        raise ValueError(f"output already exists; choose a new package directory: {output}")
    if output == target_dir or output in target_dir.parents or target_dir in output.parents:
        raise ValueError("output and target directories must not overlap")
    package = tomllib.loads((ROOT / "plugins/cmux/Cargo.toml").read_text())["package"]
    if package["name"] != "ahu-cmux-plugin":
        raise ValueError("unexpected cmux Cargo package name")

    target_dir.mkdir(parents=True, exist_ok=True)
    # Override both Cargo output locations; inherited build-dir configuration
    # must not put intermediate artifacts back into the checkout.
    env = dict(os.environ, CARGO_BUILD_BUILD_DIR=str(target_dir / "build"))
    result = subprocess.run(
        ["cargo", "build", "--locked", "--release", "--manifest-path", str(ROOT / "Cargo.toml"),
         "--package", "ahu-cmux-plugin", "--bin", "ahu-cmux-host",
         "--target-dir", str(target_dir), "--message-format=json-render-diagnostics"],
        cwd=ROOT, env=env, stdout=subprocess.PIPE, text=True, check=True,
    )
    # Cargo reports the executable location even with a configured target triple.
    executables = []
    for line in result.stdout.splitlines():
        message = json.loads(line)
        if (message.get("reason") == "compiler-artifact"
                and message["target"]["name"] == "ahu-cmux-host"
                and message.get("executable")):
            executables.append(Path(message["executable"]).resolve())
    if len(executables) != 1 or target_dir not in executables[0].parents:
        raise ValueError("Cargo did not report one ahu-cmux-host executable in the target directory")
    binary = executables[0]
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError("Cargo's ahu-cmux-host artifact is not an executable file")

    output.parent.mkdir(parents=True, exist_ok=True)
    # Assemble beside the destination, exposing the package only after success.
    with tempfile.TemporaryDirectory(prefix=".cmux-stage-", dir=output.parent) as temporary:
        staged = Path(temporary) / "package"
        (staged / "bin").mkdir(parents=True)
        shutil.copyfile(binary, staged / "bin/ahu-cmux-host")
        (staged / "bin/ahu-cmux-host").chmod(0o755)
        (staged / "ahu-plugin.md").write_text("""---
okf_version: "0.2"
type: ahu:plugin
title: cmux executable package
description: Standalone cmux session host using Host V1
tags: [session, terminal]
schema_version: 1
id: cmux-local
package: ahu-cmux-plugin
host_api: 1
cargo_manifest: Cargo.toml
executable:
  path: bin/ahu-cmux-host
  protocol_version: 1
  timeout_ms: 15000
---

# cmux executable package

Locally built Host V1 server. Installation and activation require an explicit
project binding. This package does not include the Rust source workspace.
""", encoding="utf-8")
        # Ahu discovery/install require Cargo identity and an explicit version.
        # This is metadata for the binary package, not a standalone Rust crate.
        (staged / "Cargo.toml").write_text(
            "[package]\n"
            + "".join(f"{key} = {json.dumps(package[key])}\n"
                      for key in ("name", "version", "edition", "license", "description"))
            + "publish = false\n", encoding="utf-8",
        )
        if output.exists():
            raise ValueError(f"output appeared during the build: {output}")
        staged.rename(output)
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, help="new absolute package directory outside Git checkouts")
    parser.add_argument("--target-dir", required=True, help="absolute Cargo build directory outside Git checkouts (reusable)")
    args = parser.parse_args()
    try:
        output = stage(args.output, args.target_dir)
    except subprocess.CalledProcessError as error:
        parser.exit(1, f"stage-cmux: Cargo build failed (exit {error.returncode}); package not staged\n")
    except (OSError, ValueError) as error:
        parser.exit(1, f"stage-cmux: {error}\n")
    print(output)


if __name__ == "__main__":
    main()
