#!/usr/bin/env python3
"""Run the actual checkout's ahu library parser; no agent or provider is invoked."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parent.parent


def main():
    # Cargo manifests, dependency lock, build artifacts and output stay outside Git.
    with tempfile.TemporaryDirectory(prefix='decision-grading-parser-') as temp:
        temp = Path(temp)
        manifest = ('[package]\nname = "decision-grading-parser-check"\n'
                    'version = "0.0.0"\nedition = "2024"\n\n'
                    '[dependencies]\nahu = { path = ' + json.dumps(str(REPO)) + ' }\n'
                    'serde_json = "1"\n\n[[bin]]\nname = "check"\npath = "main.rs"\n')
        (temp / 'Cargo.toml').write_text(manifest)
        shutil.copyfile(ROOT / 'check_ahu.rs', temp / 'main.rs')
        env = os.environ.copy()
        env['CARGO_TARGET_DIR'] = str(temp / 'target')
        subprocess.run(['cargo', 'run', '--offline', '--quiet', '--manifest-path',
                        str(temp / 'Cargo.toml'), '--', str(ROOT)], cwd=temp, env=env, check=True)


if __name__ == '__main__':
    main()
