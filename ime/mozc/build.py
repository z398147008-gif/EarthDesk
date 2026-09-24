"""Build EarthDesk's Mozc library (earthdesk_mozc.dll / libearthdesk_mozc.so).

Run from a checkout of https://github.com/google/mozc (the GitHub workflow
.github/workflows/mozc.yml does this on a Windows runner):

    python ime/mozc/build.py <path to mozc checkout> [--out DIR]

It copies earthdesk_mozc.cc into mozc/src/session/, adds one Bazel target
to session/BUILD.bazel (idempotent), builds it with bazelisk and copies the
library to --out (default: ime/mozc/out).
"""

import argparse
import collections
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


def annotate(lines) -> None:
    """On GitHub Actions, put the failure into an error annotation: those
    can be read without signing in (the raw log cannot)."""
    if not os.environ.get("GITHUB_ACTIONS"):
        return
    text = "\n".join(lines)[-30000:]
    text = text.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
    print(f"::error title=earthdesk_mozc build failed::{text}", flush=True)


def run_logged(cmd, cwd) -> int:
    """Run, echo everything, and on failure annotate the errors and the tail."""
    tail = collections.deque(maxlen=60)
    errors = []
    try:
        p = subprocess.Popen(cmd, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    except OSError as e:
        annotate([f"cannot run {cmd[0]}: {e}"])
        return 1
    for raw in p.stdout:
        line = raw.decode("utf-8", "replace").rstrip()
        print(line, flush=True)
        tail.append(line)
        if ("ERROR" in line or "error:" in line or "error " in line.lower()[:12]) and len(errors) < 60:
            errors.append(line)
    code = p.wait()
    if code != 0:
        annotate(["--- errors ---", *errors, "--- last lines ---", *tail])
    return code


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
    # On Windows the launcher may be bazelisk.exe, .cmd or .bat: resolve it
    # the way cmd.exe would (a bare name only finds .exe).
    bazel = shutil.which(args.bazel) or shutil.which("bazel")
    if bazel is None:
        annotate([f"{args.bazel} not found on PATH"])
        return 1
    cmd = [bazel, "build", f"//session:{name}", "--config", config, "--config", "release_build"]
    print(" ".join(cmd), flush=True)
    code = run_logged(cmd, src)
    if code != 0:
        return code
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src / "bazel-bin" / "session" / name, out / name)
    print(f"-> {out / name}")
    return 0


if __name__ == "__main__":
    # The Windows console code page cannot print everything Bazel says.
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass
    try:
        sys.exit(main())
    except SystemExit:
        raise
    except BaseException:
        import traceback

        annotate(traceback.format_exc().splitlines())
        raise
