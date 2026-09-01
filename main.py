"""Entry point for the Windows custom cursor overlay.

Examples:
    python main.py
    python main.py --style crosshair --size 60 --color "#00E5FF"
    python main.py --monitor 0 --fps 120
"""

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

    overlay = CursorOverlay(settings)
    overlay.show()
    overlay.finalize()  # Win32 click-through styles + global hotkey

    try:
        return app.exec()
    finally:
        # Always restore the system / pen cursor when the app exits (including
        # Ctrl+C in the console).
        overlay.restore_system_cursor()


if __name__ == "__main__":
    sys.exit(main())
