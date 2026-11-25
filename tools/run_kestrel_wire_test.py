#!/usr/bin/env python3
"""Server-side cross-project wire runner for v0ntr3n/cobalt-relay.

Compiles v0ntr3n/kestrel-canvas's C++17 client stack (CNetwork, CConnection, LCPacket,
BtDecryptHook, AtomicHotUpdater) from a local sibling checkout (or ephemeral git clone)
and runs all 14 strict gameplay + cold-WAL-restart tests against cobalt-relay's `gserver`.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Optional


def resolve_kestrel_dir(explicit_dir: Optional[str], relay_root: Path, tmp_root: Path, auto_clone: bool) -> Optional[Path]:
    candidates = []
    if explicit_dir:
        candidates.append(Path(explicit_dir).resolve())
    if os.environ.get("KESTREL_CANVAS_DIR"):
        candidates.append(Path(os.environ["KESTREL_CANVAS_DIR"]).resolve())
    candidates.extend([
        (relay_root.parent / "kestrel-canvas").resolve(),
        (relay_root.parent / "repo").resolve(),
    ])
    for c in candidates:
        if (c / "tests" / "test_cobalt_relay_e2e.cpp").is_file():
            return c

    if auto_clone and shutil.which("git"):
        url = os.environ.get("KESTREL_CANVAS_GIT_URL", "https://github.com/v0ntr3n/kestrel-canvas.git")
        dst = tmp_root / "kestrel-canvas"
        res = subprocess.run(["git", "clone", "--depth", "1", url, str(dst)], capture_output=True, text=True)
        if res.returncode == 0 and (dst / "tests" / "test_cobalt_relay_e2e.cpp").is_file():
            return dst
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description="Compile kestrel-canvas C++17 wire suite & test cobalt-relay")
    parser.add_argument("--kestrel-dir", default=None, help="Path to kestrel-canvas checkout")
    parser.add_argument("--gserver-bin", default=None, help="Path to compiled gserver binary")
    parser.add_argument("--auto-clone-kestrel", action="store_true", help="Clone v0ntr3n/kestrel-canvas if absent")
    args = parser.parse_args()

    relay_root = Path(__file__).resolve().parent.parent
    exe_name = "gserver.exe" if os.name == "nt" else "gserver"
    gserver_bin = (
        Path(args.gserver_bin).resolve()
        if args.gserver_bin
        else (relay_root / "target" / "release" / exe_name)
    )
    if not gserver_bin.is_file():
        debug_bin = relay_root / "target" / "debug" / exe_name
        if debug_bin.is_file():
            gserver_bin = debug_bin
        else:
            cargo = shutil.which("cargo") or str(Path.home() / ".cargo" / "bin" / "cargo")
            subprocess.run([cargo, "build", "--release", "-p", "gserver", "--offline"], cwd=str(relay_root), check=True)

    cxx = shutil.which("g++") or shutil.which("clang++")
    if not cxx:
        print("[FAIL] No C++17 compiler (g++ / clang++) found on PATH.", file=sys.stderr)
        return 1

    build_dir = relay_root / "target" / "kestrel_wire_e2e"
    build_dir.mkdir(parents=True, exist_ok=True)
    out_bin = build_dir / ("test_cobalt_relay_e2e.exe" if os.name == "nt" else "test_cobalt_relay_e2e")

    with tempfile.TemporaryDirectory(prefix="cobalt_kestrel_wire_") as tmp_s:
        kestrel_dir = resolve_kestrel_dir(args.kestrel_dir, relay_root, Path(tmp_s), args.auto_clone_kestrel)
        if kestrel_dir is None:
            print("[FAIL] Could not locate kestrel-canvas checkout with tests/test_cobalt_relay_e2e.cpp", file=sys.stderr)
            return 1

        compile_cmd = [
            cxx,
            "-std=c++17",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-DGCLIENT_STANDALONE_BUILD=1",
            f"-I{kestrel_dir / 'src'}",
            str(kestrel_dir / "tests" / "test_cobalt_relay_e2e.cpp"),
            str(kestrel_dir / "src" / "assets" / "BtDecryptHook.cpp"),
            str(kestrel_dir / "src" / "net" / "LCPacket.cpp"),
            str(kestrel_dir / "src" / "net" / "CConnection.cpp"),
            str(kestrel_dir / "src" / "net" / "CNetwork.cpp"),
            str(kestrel_dir / "src" / "update" / "AtomicHotUpdater.cpp"),
            "-lz",
            "-lcrypto",
            "-lpthread",
            "-o",
            str(out_bin),
        ]
        print(f"[cobalt-wire] Compiling kestrel-canvas C++17 wire client from {kestrel_dir} ...", flush=True)
        subprocess.run(compile_cmd, check=True)

        runner = kestrel_dir / "tools" / "run_cobalt_relay_e2e.py"
        subprocess.run(
            [
                sys.executable,
                str(runner),
                "--client-test-bin",
                str(out_bin),
                "--gserver-bin",
                str(gserver_bin),
                "--cobalt-relay-dir",
                str(relay_root),
                "--require-relay",
            ],
            check=True,
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
