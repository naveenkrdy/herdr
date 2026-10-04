from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "install-local.sh"

# The binary the fake cargo "builds": it answers `status server --json` from
# FAKE_SERVER and fails `server live-handoff` when FAKE_HANDOFF_FAIL is 1, and
# records each call with the socket overrides it saw.
FAKE_HERDR = """#!/bin/sh
echo "$* socket=${HERDR_SOCKET_PATH-unset} client=${HERDR_CLIENT_SOCKET_PATH-unset}" >> "$FAKE_CALLS"
case "$1 $2" in
  "status server")
    case "${FAKE_SERVER:-}" in
      running) echo '{"status":"running","running":true}' ;;
      broken) echo "permission denied" >&2; exit 1 ;;
      *) echo '{"status":"not running","running":false}' ;;
    esac
    ;;
  "server live-handoff")
    echo "handoff output"
    [ "${FAKE_HANDOFF_FAIL:-}" = 1 ] && exit 1
    exit 0
    ;;
esac
"""


@unittest.skipUnless(os.name == "posix", "install-local.sh requires a POSIX host")
class InstallLocalTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory(prefix="herdr-install-local-test-")
        self.root = Path(self.temp_dir.name)
        self.repo = self.root / "repo"
        (self.repo / "scripts").mkdir(parents=True)
        shutil.copy2(SCRIPT, self.repo / "scripts" / "install-local.sh")
        self._git("init", "-q")
        self._git("add", ".")
        self._git("commit", "-q", "-m", "init")
        self.sha = self._git("rev-parse", "--short=8", "HEAD").strip()

        self.stubs = self.root / "stubs"
        self.stubs.mkdir()
        self.calls = self.root / "calls.log"
        self.install_dir = self.root / "opt" / "herdr"
        self.bin_dir = self.root / "bin"
        self.new_bin = self.install_dir / f"herdr-{self.sha}"
        self.link = self.bin_dir / "herdr"

        (self.root / "fake-herdr").write_text(FAKE_HERDR)
        self._stub(
            "cargo",
            f"""#!/bin/sh
echo "cargo $*" >> "$FAKE_CALLS"
mkdir -p target/release
cp "{self.root / 'fake-herdr'}" target/release/herdr
chmod 755 target/release/herdr
""",
        )
        self._stub("zig", '#!/bin/sh\n[ "$1" = version ] && echo "${FAKE_ZIG_VERSION:-0.16.0}"\n')

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def _git(self, *args: str) -> str:
        return subprocess.run(
            ["git", "-c", "user.name=t", "-c", "user.email=t@example.com", *args],
            cwd=self.repo,
            check=True,
            capture_output=True,
            text=True,
        ).stdout

    def _stub(self, name: str, body: str) -> None:
        path = self.stubs / name
        path.write_text(body)
        path.chmod(0o755)

    def _run(self, *args: str, **env: str) -> subprocess.CompletedProcess[str]:
        git_dir = str(Path(shutil.which("git") or "/usr/bin/git").parent)
        full_env = {
            "PATH": os.pathsep.join([str(self.stubs), git_dir, "/usr/bin", "/bin"]),
            "HOME": str(self.root / "home"),
            "TMPDIR": str(self.root),
            "HERDR_INSTALL_DIR": str(self.install_dir),
            "HERDR_BIN_DIR": str(self.bin_dir),
            "HERDR_SOCKET_PATH": "/outer/herdr.sock",
            "HERDR_CLIENT_SOCKET_PATH": "/outer/herdr-client.sock",
            "FAKE_CALLS": str(self.calls),
            **env,
        }
        return subprocess.run(
            ["bash", str(self.repo / "scripts" / "install-local.sh"), *args],
            env=full_env,
            capture_output=True,
            text=True,
        )

    def _calls(self) -> list[str]:
        return self.calls.read_text().splitlines() if self.calls.exists() else []

    def _old_link(self) -> Path:
        old = self.install_dir / "herdr-old"
        self.install_dir.mkdir(parents=True)
        old.write_text("old")
        self.bin_dir.mkdir(parents=True)
        self.link.symlink_to(old)
        return old

    def test_no_server_and_no_folders_copies_and_links_without_a_handoff(self) -> None:
        result = self._run()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(os.access(self.new_bin, os.X_OK))
        self.assertEqual(os.readlink(self.link), str(self.new_bin))
        self.assertIn("cargo build --release --locked", self._calls())
        self.assertFalse(any("live-handoff" in call for call in self._calls()))

    def test_running_server_is_handed_over_then_the_link_moves(self) -> None:
        self._old_link()

        result = self._run(FAKE_SERVER="running")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            f"server live-handoff --import-exe {self.new_bin} socket=unset client=unset",
            self._calls(),
        )
        self.assertIn("status server --json socket=unset client=unset", self._calls())
        log = self.install_dir / "logs" / f"handoff-herdr-{self.sha}.log"
        self.assertIn("handoff output", log.read_text())
        self.assertEqual(os.readlink(self.link), str(self.new_bin))

    def test_failed_handoff_keeps_the_old_link_and_fails(self) -> None:
        old = self._old_link()

        result = self._run(FAKE_SERVER="running", FAKE_HANDOFF_FAIL="1")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("live handoff failed", result.stderr)
        self.assertEqual(len(result.stderr.strip().splitlines()), 1, result.stderr)
        self.assertEqual(os.readlink(self.link), str(old))

    def test_unreadable_server_status_keeps_the_old_link_and_fails(self) -> None:
        old = self._old_link()

        result = self._run(FAKE_SERVER="broken")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("cannot tell whether a Herdr server runs: permission denied", result.stderr)
        self.assertEqual(os.readlink(self.link), str(old))

    def test_wrong_zig_stops_before_building(self) -> None:
        result = self._run(FAKE_ZIG_VERSION="0.15.2")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("needs Zig 0.16.0, found 0.15.2", result.stderr)
        self.assertEqual(self._calls(), [])
        self.assertFalse(self.link.exists())

    def test_missing_zig_names_what_to_install(self) -> None:
        result = self._run(ZIG=str(self.root / "no-such-zig"))

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("needs Zig 0.16.0", result.stderr)

    def test_same_commit_already_copied_reuses_the_copy(self) -> None:
        self.assertEqual(self._run().returncode, 0)
        self.new_bin.write_text(self.new_bin.read_text() + "# first copy\n")
        self.calls.unlink()

        result = self._run(FAKE_ZIG_VERSION="0.15.2")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("# first copy", self.new_bin.read_text())
        self.assertEqual(self._calls(), [])
        self.assertEqual(os.readlink(self.link), str(self.new_bin))

    def test_uncommitted_changes_build_a_dirty_copy(self) -> None:
        (self.repo / "scripts" / "install-local.sh").write_text(
            (self.repo / "scripts" / "install-local.sh").read_text() + "\n"
        )

        result = self._run()

        self.assertEqual(result.returncode, 0, result.stderr)
        dirty = self.install_dir / f"herdr-{self.sha}-dirty"
        self.assertEqual(os.readlink(self.link), str(dirty))

    def test_arguments_are_refused(self) -> None:
        result = self._run("--force")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("takes no arguments", result.stderr)


if __name__ == "__main__":
    unittest.main()
