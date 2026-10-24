#!/usr/bin/env python3
"""End-to-end scenarios: the real `lf-mide` in a pseudo terminal.

    cargo build && python3 tests/e2e/run.py            # all scenarios
    python3 tests/e2e/run.py Scenarios.test_help       # one scenario

Nothing here touches your real config, projects or devices: each scenario
gets its own home folder, a throwaway project, and (for builds) shim
`gradlew` / `adb` / `emulator` scripts. The leader is F12 because Ctrl-Space
is often taken by the system.
"""

import faulthandler
import os
import stat
import subprocess
import sys
import tempfile
import unittest
import warnings
from pathlib import Path

from harness import CMD_SHIFT_O, ESC, F12, App

BUILD_FILE = """
android {
    defaultConfig { applicationId = "com.example.fake" }
    productFlavors {
        create("free")
        create("paid")
    }
}
"""

# Prints what it was asked to do, then ends (or hangs, to be cancelled).
GRADLEW = """#!/bin/sh
echo "fake gradle: $@"
if [ -n "$FAKE_GRADLE_HANG" ]; then sleep 300; fi
"""

ADB = """#!/bin/sh
case "$*" in
  "devices -l") printf 'List of devices attached\\nemulator-5554 device product:sdk model:Pixel_8 device:emu\\n' ;;
  *"emu avd name"*) printf 'Pixel_8\\nOK\\n' ;;
  *"pm path"*) echo 'package:/data/app/fake.apk' ;;
  *pidof*) echo 4242 ;;
  *logcat*) echo '10-24 12:00:00.000  4242  4242 I Fake: hello from logcat'; sleep 300 ;;
  *) ;;
esac
"""

EMULATOR = """#!/bin/sh
case "$1" in -list-avds) echo Pixel_8 ;; esac
"""


def write_script(path: Path, text: str):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


def make_project(root: Path) -> Path:
    project = root / "demo"
    (project / "iosApp").mkdir(parents=True)
    (project / "composeApp" / "src").mkdir(parents=True)
    (project / "settings.gradle.kts").write_text('rootProject.name = "demo"\n')
    (project / "composeApp" / "build.gradle.kts").write_text(BUILD_FILE)
    (project / "composeApp" / "src" / "Main.kt").write_text("fun main() {}\n")
    (project / "NOTES.md").write_text("notes\n")
    write_script(project / "gradlew", GRADLEW)
    subprocess.run(["git", "init", "-q", str(project)], check=True)
    return project


def make_sdk(root: Path) -> Path:
    sdk = root / "sdk"
    write_script(sdk / "platform-tools" / "adb", ADB)
    write_script(sdk / "emulator" / "emulator", EMULATOR)
    return sdk


class Scenarios(unittest.TestCase):
    def setUp(self):
        self._scratch = tempfile.TemporaryDirectory(prefix="lf-mide-e2e-project-")
        self.addCleanup(self._scratch.cleanup)
        scratch = Path(self._scratch.name)
        self.project = make_project(scratch)
        self.sdk = make_sdk(scratch)
        self.apps = []

    def tearDown(self):
        for app in self.apps:
            app.close()

    def start(self, config='leader = "F12"\n', env=None, ready="Devices") -> App:
        app = App(self.project, config=config, extra_env=env)
        self.apps.append(app)
        app.wait_for(ready, timeout=15)
        return app

    def start_with_sdk(self, **env) -> App:
        app = self.start(env={"ANDROID_HOME": str(self.sdk), **env})
        app.wait_for("Pixel_8", timeout=15)
        return app

    # --- keys and overlays -------------------------------------------------

    def test_the_leader_then_q_quits(self):
        app = self.start()

        app.send(F12)
        app.send("q")

        self.assertEqual(app.wait_exit(), 0)

    def test_help_opens_after_the_leader_and_closes_with_escape(self):
        app = self.start()

        app.send(F12)
        app.send("?")
        text = app.wait_for("After the leader")
        app.send(ESC)

        self.assertIn("Vim basics", text)
        app.wait_gone("After the leader")

    def test_cmd_shift_o_opens_the_finder_which_lists_project_files(self):
        app = self.start()

        app.send(CMD_SHIFT_O)
        text = app.wait_for("Find file")
        app.send(ESC)

        self.assertIn("files · type to filter", text)
        app.wait_gone("Find file")

    def test_a_bare_q_does_not_quit_but_the_leader_q_does(self):
        app = self.start()

        app.send("q")
        app.pump(0.5)
        self.assertFalse(app.exited)
        app.send(F12)
        app.send("q")

        self.assertEqual(app.wait_exit(), 0)

    def test_a_key_that_is_not_a_command_releases_the_leader(self):
        app = self.start()

        app.send(F12)
        app.send("z")
        app.send("q")
        app.pump(0.5)

        self.assertFalse(app.exited, "q after an unknown key is only a bare q")

    # --- variants ----------------------------------------------------------

    def test_the_variant_picker_lists_flavors_and_remembers_the_choice(self):
        app = self.start()

        app.send(F12)
        app.send("v")
        text = app.wait_for("freeDebug")
        app.send("j")
        app.send("\r")

        self.assertIn("paidDebug", text)
        app.wait_for("Build variant: paidDebug")

    # --- build and run with shim tools -------------------------------------

    def test_a_run_installs_the_chosen_variant_and_follows_only_the_app(self):
        app = self.start_with_sdk()
        app.send(F12)
        app.send("v")
        app.wait_for("freeDebug")
        app.send("\r")
        app.wait_for("Build variant: freeDebug")

        app.send(F12)
        app.send("B")
        app.wait_for("built and running", timeout=20)
        app.wait_for("hello from logcat", timeout=10)
        app.send(F12)
        app.send("o")
        app.send("1")

        app.wait_for(":composeApp:installFreeDebug")

    def test_cancel_stops_a_build_that_is_stuck(self):
        app = self.start_with_sdk(FAKE_GRADLE_HANG="1")

        app.send(F12)
        app.send("B")
        app.wait_for("fake gradle", timeout=15)
        app.wait_for(" · 1s", timeout=5)  # the build clock is running
        app.send(F12)
        app.send("s")

        app.wait_for("build cancelled", timeout=10)


if __name__ == "__main__":
    # forkpty warns because the test runner has threads; the child only execs.
    warnings.filterwarnings("ignore", category=DeprecationWarning, module="pty")
    # A hung scenario ends the run with a traceback instead of waiting forever.
    faulthandler.dump_traceback_later(int(os.environ.get("LF_MIDE_E2E_TIMEOUT", "600")), exit=True)
    sys.path.insert(0, str(Path(__file__).parent))
    unittest.main(verbosity=2)
