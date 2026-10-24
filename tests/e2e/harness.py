"""Drives the real `lf-mide` binary in a pseudo terminal.

Python standard library only. `Screen` is a small terminal emulator that
understands what ratatui/crossterm write (cursor moves, erase, SGR), so
tests can assert on what is really on screen rather than on raw bytes.
"""

import fcntl
import os
import pty
import re
import select
import shutil
import signal
import struct
import tempfile
import termios
import time
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BINARY = Path(os.environ.get("LF_MIDE_BIN", ROOT / "target" / "debug" / "lf-mide"))
ROWS, COLS = 45, 160
CSI = re.compile(rb"\x1b\[([0-9;?<>=]*)([ -/]*)([@-~])")
OSC = re.compile(rb"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")
ESC_OTHER = re.compile(rb"\x1b[()][A-Z0-9]|\x1b[=>78]")


class Screen:
    """The text grid after replaying a stream of terminal output."""

    def __init__(self, rows=ROWS, cols=COLS):
        self.rows, self.cols = rows, cols
        self.grid = [[" "] * cols for _ in range(rows)]
        self.row = self.col = 0
        self.tail = b""

    def feed(self, data: bytes):
        data = OSC.sub(b"", self.tail + data)
        self.tail = b""
        position = 0
        while position < len(data):
            byte = data[position]
            if byte == 0x1B:
                match = CSI.match(data, position)
                if match:
                    self._csi(match.group(1).decode(), match.group(3).decode())
                    position = match.end()
                    continue
                other = ESC_OTHER.match(data, position)
                if other:
                    position = other.end()
                    continue
                self.tail = data[position:]  # an incomplete sequence
                return
            if byte == 0x0D:
                self.col = 0
            elif byte == 0x0A:
                self.row = min(self.row + 1, self.rows - 1)
            elif byte == 0x08:
                self.col = max(self.col - 1, 0)
            elif byte < 0x20:
                pass
            else:
                length = 1 if byte < 0x80 else (2 if byte < 0xE0 else 3 if byte < 0xF0 else 4)
                chunk = data[position : position + length]
                if len(chunk) < length:
                    self.tail = data[position:]
                    return
                self._put(chunk.decode("utf8", "replace"))
                position += length
                continue
            position += 1

    def _put(self, char):
        width = 2 if unicodedata.east_asian_width(char) in "WF" else 1
        if unicodedata.combining(char):
            return
        if self.col + width > self.cols:
            return
        self.grid[self.row][self.col] = char
        if width == 2 and self.col + 1 < self.cols:
            self.grid[self.row][self.col + 1] = ""
        self.col += width

    def _csi(self, params, final):
        if params[:1] in "?<>=":
            return
        numbers = [int(p) if p else 0 for p in params.split(";")] if params else []
        first = numbers[0] if numbers and numbers[0] else 1
        if final in "Hf":
            self.row = min(max((numbers[0] if numbers and numbers[0] else 1) - 1, 0), self.rows - 1)
            self.col = min(max((numbers[1] if len(numbers) > 1 and numbers[1] else 1) - 1, 0), self.cols - 1)
        elif final == "A":
            self.row = max(self.row - first, 0)
        elif final == "B":
            self.row = min(self.row + first, self.rows - 1)
        elif final == "C":
            self.col = min(self.col + first, self.cols - 1)
        elif final == "D":
            self.col = max(self.col - first, 0)
        elif final == "G":
            self.col = min(first - 1, self.cols - 1)
        elif final == "d":
            self.row = min(first - 1, self.rows - 1)
        elif final == "J":
            mode = numbers[0] if numbers else 0
            if mode in (2, 3):
                self.grid = [[" "] * self.cols for _ in range(self.rows)]
            elif mode == 0:
                self.grid[self.row][self.col :] = [" "] * (self.cols - self.col)
                for r in range(self.row + 1, self.rows):
                    self.grid[r] = [" "] * self.cols
        elif final == "K":
            mode = numbers[0] if numbers else 0
            if mode == 0:
                self.grid[self.row][self.col :] = [" "] * (self.cols - self.col)
            elif mode == 2:
                self.grid[self.row] = [" "] * self.cols
        elif final == "X":
            for c in range(self.col, min(self.col + first, self.cols)):
                self.grid[self.row][c] = " "

    def text(self) -> str:
        return "\n".join("".join(row).rstrip() for row in self.grid)


class App:
    """A running `lf-mide` with its own home, config and project."""

    def __init__(self, project: Path, config: str = 'leader = "F12"\n', extra_env=None):
        if not BINARY.exists():
            raise FileNotFoundError(f"{BINARY} not found: run `cargo build` first")
        self.home = Path(tempfile.mkdtemp(prefix="lf-mide-e2e-"))
        config_dir = self.home / "config" / "lf-mide"
        config_dir.mkdir(parents=True)
        (config_dir / "config.toml").write_text(config)
        env = dict(os.environ)
        env.update(
            HOME=str(self.home),
            XDG_CONFIG_HOME=str(self.home / "config"),
            XDG_CACHE_HOME=str(self.home / "cache"),
            TERM="xterm-256color",
        )
        for name in ("ANDROID_HOME", "ANDROID_SDK_ROOT", "TMUX"):
            env.pop(name, None)
        env.update(extra_env or {})
        self.screen = Screen()
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(project)
            os.execve(str(BINARY), [str(BINARY)], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        self.exited = False

    def pump(self, seconds=0.2):
        end = time.time() + seconds
        while time.time() < end:
            ready, _, _ = select.select([self.fd], [], [], 0.05)
            if not ready:
                continue
            try:
                data = os.read(self.fd, 65536)
            except OSError:
                self.exited = True
                return
            if not data:
                self.exited = True
                return
            self.screen.feed(data)

    def send(self, data):
        os.write(self.fd, data if isinstance(data, bytes) else data.encode())
        self.pump(0.3)

    def wait_for(self, needle: str, timeout=10.0) -> str:
        end = time.time() + timeout
        while time.time() < end:
            self.pump(0.1)
            if needle in self.screen.text():
                return self.screen.text()
        raise AssertionError(f"{needle!r} never appeared; screen:\n{self.screen.text()}")

    def wait_gone(self, needle: str, timeout=5.0):
        end = time.time() + timeout
        while time.time() < end:
            self.pump(0.1)
            if needle not in self.screen.text():
                return
        raise AssertionError(f"{needle!r} did not go away; screen:\n{self.screen.text()}")

    def wait_exit(self, timeout=10.0) -> int:
        end = time.time() + timeout
        while time.time() < end:
            self.pump(0.1)
            done, status = os.waitpid(self.pid, os.WNOHANG)
            if done:
                self.exited = True
                return os.waitstatus_to_exitcode(status)
        raise AssertionError(f"lf-mide did not exit; screen:\n{self.screen.text()}")

    def close(self):
        if not self.exited:
            try:
                os.kill(self.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            # Keep reading: an app blocked on a full terminal cannot quit.
            end = time.time() + 5
            while time.time() < end:
                self.pump(0.1)
                try:
                    done, _ = os.waitpid(self.pid, os.WNOHANG)
                except ChildProcessError:
                    break
                if done:
                    break
            else:
                try:
                    os.kill(self.pid, signal.SIGKILL)
                    os.waitpid(self.pid, 0)
                except (ProcessLookupError, ChildProcessError):
                    pass
        try:
            os.close(self.fd)
        except OSError:
            pass
        shutil.rmtree(self.home, ignore_errors=True)


F12 = b"\x1b[24~"
ESC = b"\x1b"
CMD_SHIFT_O = b"\x1b[111;10u"
