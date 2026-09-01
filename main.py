"""Entry point for the Windows custom cursor overlay.

Examples:
    python main.py
    python main.py --style crosshair --size 60 --color "#00E5FF"
    python main.py --monitor 0 --fps 120
"""

import ctypes
import sys

from PySide6.QtWidgets import QApplication

from overlay import CursorOverlay
from settings import parse_args


def main() -> int:
    settings = parse_args()

    app = QApplication(sys.argv)
    # Keep running even if the (invisible) overlay would otherwise count as
    # the last window.
    app.setQuitOnLastWindowClosed(False)

    # Raise the Windows timer resolution to 1 ms so the high-FPS poll timer
    # (200 Hz by default) is not clamped to the default ~15.6 ms.
    winmm = ctypes.windll.winmm
    winmm.timeBeginPeriod(1)

    overlay = CursorOverlay(settings)
    overlay.show()
    overlay.finalize()  # Win32 click-through styles + global hotkey

    try:
        return app.exec()
    finally:
        # Always restore the system / pen cursor when the app exits (including
        # Ctrl+C in the console).
        overlay.restore_system_cursor()
        winmm.timeEndPeriod(1)


if __name__ == "__main__":
    sys.exit(main())


if __name__ == "__main__":
    sys.exit(main())
