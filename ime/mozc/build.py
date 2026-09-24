"""Build EarthDesk's Mozc library (earthdesk_mozc.dll / libearthdesk_mozc.so).

Run from a checkout of https://github.com/google/mozc (the GitHub workflow
.github/workflows/mozc.yml does this on a Windows runner):

    python ime/mozc/build.py <path to mozc checkout> [--out DIR]

It copies earthdesk_mozc.cc into mozc/src/session/, adds one Bazel target
to session/BUILD.bazel (idempotent), builds it with bazelisk and copies the
library to --out (default: ime/mozc/out).
"""

import argparse
import os
import pathlib
import platform
import shutil
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
MARK = "# --- EarthDesk ---"


def target_name() -> str:
    return "earthdesk_mozc.dll" if os.name == "nt" else "libearthdesk_mozc.so"


def patch(src: pathlib.Path) -> None:
    shutil.copy2(HERE / "earthdesk_mozc.cc", src / "session" / "earthdesk_mozc.cc")
    build = src / "session" / "BUILD.bazel"
    text = build.read_text(encoding="utf-8")
    if MARK in text:
        text = text[: text.index(MARK)]
    text += f"""{MARK}
# Mozc's conversion engine as a C library for 地球桌面输入法 (see
# earthdesk_mozc.cc). Added by EarthDesk's ime/mozc/build.py.
[mozc_cc_binary(
    name = n,
    srcs = ["earthdesk_mozc.cc"],
    linkshared = True,
    deps = [
        ":session_handler",
        "//base:system_util",
        "//engine:engine_factory",
        "//protocol:commands_cc_proto",
    ],
) for n in ["earthdesk_mozc.dll", "libearthdesk_mozc.so"]]
"""
    build.write_text(text, encoding="utf-8")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("mozc")
    ap.add_argument("--out", default=str(HERE / "out"))
    ap.add_argument("--bazel", default="bazelisk")
    args = ap.parse_args()
    src = pathlib.Path(args.mozc).resolve() / "src"
    patch(src)
    name = target_name()
    config = "oss_windows" if os.name == "nt" else "oss_linux"
    cmd = [args.bazel, "build", f"//session:{name}", "--config", config, "--config", "release_build"]
    print(" ".join(cmd), flush=True)
    r = subprocess.run(cmd, cwd=src)
    if r.returncode != 0:
        return r.returncode
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src / "bazel-bin" / "session" / name, out / name)
    print(f"-> {out / name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
