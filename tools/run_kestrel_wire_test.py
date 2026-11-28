#!/usr/bin/env python3
"""Server-side cross-project wire runner for v0ntr3n/cobalt-relay.

Compiles v0ntr3n/kestrel-canvas's C++17/C++20 client stack (CNetwork, CConnection, LCPacket,
BtDecryptHook, AtomicHotUpdater, and optional LuaJIT 2.1 Amf3Module) from a local sibling
checkout (including `feat/axmol-port` or `main`) and runs cross-project verification
against cobalt-relay's `gserver`.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import List, Optional, Tuple


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
        ref = os.environ.get("KESTREL_CANVAS_GIT_REF", "feat/axmol-port")
        dst = tmp_root / "kestrel-canvas"
        res = subprocess.run(
            ["git", "clone", "--depth", "1", "--branch", ref, url, str(dst)],
            capture_output=True,
            text=True,
        )
        if res.returncode != 0:
            shutil.rmtree(dst, ignore_errors=True)
            res = subprocess.run(["git", "clone", "--depth", "1", url, str(dst)], capture_output=True, text=True)
        if res.returncode == 0 and (dst / "tests" / "test_cobalt_relay_e2e.cpp").is_file():
            return dst
    return None


def detect_luajit_flags(kestrel_dir: Path) -> Tuple[List[str], List[str], bool]:
    """Resolves LuaJIT 2.1 / Lua 5.1 include & link flags required by feat/axmol-port CConnection.cpp."""
    pkg_cfg = shutil.which("pkg-config")
    if pkg_cfg:
        for pkg in ("luajit", "lua5.1", "lua-5.1"):
            cflags = subprocess.run([pkg_cfg, "--cflags", pkg], capture_output=True, text=True)
            libs = subprocess.run([pkg_cfg, "--libs", pkg], capture_output=True, text=True)
            if cflags.returncode == 0 and libs.returncode == 0:
                return cflags.stdout.split(), libs.stdout.split(), (pkg == "luajit")

    inc_candidates = [
        Path("/usr/include/luajit-2.1"),
        Path("/usr/local/include/luajit-2.1"),
        kestrel_dir / "third_party" / "luajit" / "src",
    ]
    if os.environ.get("AX_ROOT"):
        ax = Path(os.environ["AX_ROOT"])
        inc_candidates.extend([
            ax / "3rdparty" / "lua" / "luajit" / "include",
            ax / "3rdparty" / "luajit" / "include",
        ])
    if os.environ.get("VCPKG_INSTALLATION_ROOT"):
        vp = Path(os.environ["VCPKG_INSTALLATION_ROOT"]) / "installed" / "x64-windows"
        inc_candidates.append(vp / "include" / "luajit")

    for inc in inc_candidates:
        if (inc / "lua.h").is_file():
            return [f"-I{inc}"], ["-lluajit-5.1"], True

    return [], [], False


def main() -> int:
    parser = argparse.ArgumentParser(description="Compile kestrel-canvas C++ wire suite & test cobalt-relay")
    parser.add_argument("--kestrel-dir", default=None, help="Path to kestrel-canvas checkout")
    parser.add_argument("--gserver-bin", default=None, help="Path to compiled gserver binary")
    parser.add_argument("--auto-clone-kestrel", action="store_true", help="Clone v0ntr3n/kestrel-canvas if absent")
    parser.add_argument("--force-live", action="store_true", help="Force live RPC stages even across mismatched feature branches")
    parser.add_argument(
        "--skip-if-missing",
        action="store_true",
        help="Exit 0 cleanly when sibling kestrel-canvas checkout or C++ compiler is unavailable",
    )
    args = parser.parse_args()

    relay_root = Path(__file__).resolve().parent.parent
    cxx = shutil.which("g++") or shutil.which("clang++")
    if not cxx:
        if args.skip_if_missing:
            print("[SKIP] No C++ compiler (g++ / clang++) found on PATH; skipping kestrel wire test.", flush=True)
            return 0
        print("[FAIL] No C++ compiler (g++ / clang++) found on PATH.", file=sys.stderr)
        return 1

    with tempfile.TemporaryDirectory(prefix="cobalt_kestrel_wire_") as tmp_s:
        kestrel_dir = resolve_kestrel_dir(args.kestrel_dir, relay_root, Path(tmp_s), args.auto_clone_kestrel)
        if kestrel_dir is None:
            if args.skip_if_missing:
                print("[SKIP] Sibling kestrel-canvas checkout not found; skipping kestrel wire test.", flush=True)
                return 0
            print("[FAIL] Could not locate kestrel-canvas checkout with tests/test_cobalt_relay_e2e.cpp", file=sys.stderr)
            return 1

        exe_name = "gserver.exe" if os.name == "nt" else "gserver"
        target_root = (
            Path(os.environ["CARGO_TARGET_DIR"]).resolve()
            if os.environ.get("CARGO_TARGET_DIR")
            else (relay_root / "target")
        )
        gserver_bin = (
            Path(args.gserver_bin).resolve()
            if args.gserver_bin
            else (target_root / "release" / exe_name)
        )
        if not gserver_bin.is_file():
            debug_bin = target_root / "debug" / exe_name
            if debug_bin.is_file():
                gserver_bin = debug_bin
            else:
                cargo = shutil.which("cargo") or str(Path.home() / ".cargo" / "bin" / "cargo")
                subprocess.run([cargo, "build", "--release", "-p", "gserver", "--offline"], cwd=str(relay_root), check=True)

        build_dir = target_root / "kestrel_wire_e2e"
        build_dir.mkdir(parents=True, exist_ok=True)
        out_bin = build_dir / ("test_cobalt_relay_e2e.exe" if os.name == "nt" else "test_cobalt_relay_e2e")

        lua_cflags, lua_libs, has_luajit = detect_luajit_flags(kestrel_dir)
        compat_inc = kestrel_dir / "src" / "compat" / "include"
        amf3_cpp = kestrel_dir / "src" / "net" / "Amf3Module.cpp"
        test_cpp = kestrel_dir / "tests" / "test_cobalt_relay_e2e.cpp"

        compile_cmd = [
            cxx,
            "-std=c++20",
            "-fno-char8_t",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-DGCLIENT_STANDALONE_BUILD=1",
            f"-I{kestrel_dir / 'src'}",
        ]
        if compat_inc.is_dir():
            compile_cmd.append(f"-I{compat_inc}")
        compile_cmd.extend(lua_cflags)

        sources = [
            str(test_cpp),
            str(kestrel_dir / "src" / "assets" / "BtDecryptHook.cpp"),
            str(kestrel_dir / "src" / "net" / "LCPacket.cpp"),
            str(kestrel_dir / "src" / "net" / "CConnection.cpp"),
            str(kestrel_dir / "src" / "net" / "CNetwork.cpp"),
            str(kestrel_dir / "src" / "update" / "AtomicHotUpdater.cpp"),
        ]
        if has_luajit and amf3_cpp.is_file():
            compile_cmd.append("-DGCLIENT_WITH_LUAJIT_AMF3=1")
            sources.append(str(amf3_cpp))

        compile_cmd.extend(sources)
        compile_cmd.extend(lua_libs)
        compile_cmd.extend([
            "-lz",
            "-lcrypto",
            "-lpthread",
            "-ldl",
            "-o",
            str(out_bin),
        ])
        print(f"[cobalt-wire] Compiling kestrel-canvas C++20 wire client from {kestrel_dir} ...", flush=True)
        subprocess.run(compile_cmd, check=True)

        test_cpp_src = test_cpp.read_text(encoding="utf-8", errors="ignore")
        has_event_pay = (relay_root / "crates" / "gserver" / "src" / "event_pay_rpcs.rs").is_file()
        force_live = args.force_live or os.environ.get("COBALT_FORCE_LIVE_XREPO") == "1"

        if ("pay.instantBuy" in test_cpp_src) and not has_event_pay and not force_live:
            extra_fixture_args: List[str] = []
            enc_p = relay_root / "crates" / "gserver-data" / "data" / "test_vectors" / "MainTownSceneReader.lua.enc"
            dec_p = relay_root / "crates" / "gserver-data" / "data" / "test_vectors" / "MainTownSceneReader.lua.dec"
            if enc_p.is_file() and dec_p.is_file():
                extra_fixture_args = ["--enc-lua", str(enc_p), "--dec-lua", str(dec_p)]
            print(
                "[cobalt-wire] Sibling kestrel-canvas targets feat/active-events-and-instant-buy "
                "(event_pay_rpcs.rs absent on this cobalt-relay branch); running C++20 codec/wire self-tests.",
                flush=True,
            )
            subprocess.run([str(out_bin), "--self-test", *extra_fixture_args], check=True)
            return 0

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
