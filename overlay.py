"""Full-screen, click-through overlay that paints a custom cursor.

The overlay covers the whole (virtual) desktop, optionally hides the native
cursor and draws a highly visible custom cursor (ring / crosshair / dot) at
the exact pointer position.  This is especially useful with a drawing pad,
where precise pen-position feedback matters.

Windows-only (uses Win32 APIs via ctypes).
"""

import ctypes
import ctypes.wintypes

from PySide6.QtCore import QPointF, QRect, Qt, QTimer
from PySide6.QtGui import QColor, QCursor, QGuiApplication, QPainter, QScreen
from PySide6.QtWidgets import QWidget

from cursors import draw_cursor

# --- Win32 extended-window-style flags ------------------------------------
GWL_EXSTYLE = -20
WS_EX_LAYERED = 0x00080000      # enables per-pixel transparency
WS_EX_TRANSPARENT = 0x00000020  # mouse clicks pass through the window
WS_EX_TOOLWINDOW = 0x00000080   # keep it out of the taskbar / alt-tab
WS_EX_NOACTIVATE = 0x08000000   # never steal focus

# Global hotkey: Ctrl+Shift+H toggles the native Windows cursor.
MOD_CONTROL = 0x0002
MOD_SHIFT = 0x0004
VK_H = 0x48
WM_HOTKEY = 0x0312
TOGGLE_HOTKEY_ID = 1


class CursorOverlay(QWidget):
    """Translucent, always-on-top window that tracks and redraws the cursor."""

    def __init__(self, settings):
        super().__init__(None)
        self.settings = settings
        self.color = QColor(settings.color)
        self.last_pos = None
        self.native_cursor_hidden = settings.hide_system_cursor

        self._setup_window()

    # ------------------------------------------------------------- setup --
    def _setup_window(self) -> None:
        self.setWindowFlags(
            Qt.WindowType.FramelessWindowHint
            | Qt.WindowType.WindowStaysOnTopHint
            | Qt.WindowType.Tool
            | Qt.WindowType.NoDropShadowWindowHint
        )
        self.setAttribute(Qt.WidgetAttribute.WA_TranslucentBackground)
        self.setAttribute(Qt.WidgetAttribute.WA_TransparentForMouseEvents)
        self.setAttribute(Qt.WidgetAttribute.WA_ShowWithoutActivating)
        self.setGeometry(self._virtual_geometry())

        # The overlay is always the top-most window, so its own cursor shape
        # is what the user sees.  BlankCursor hides the native cursor.
        self.setCursor(
            Qt.CursorShape.BlankCursor
            if self.native_cursor_hidden
            else Qt.CursorShape.ArrowCursor
        )

        self.timer = QTimer(self)
        self.timer.setInterval(1000 // max(self.settings.fps, 1))
        self.timer.timeout.connect(self._poll)
        self.timer.start()

    def finalize(self) -> None:
        """Apply Win32 styles and register hotkeys once the HWND exists.

        Must be called after show(), because GetWindowLong/RegisterHotKey
        need a real native window handle.
        """
        self._apply_win32_styles()
        self._register_hotkey()

    def _virtual_geometry(self) -> QRect:
        """Combined geometry across all monitors (or a single one)."""
        screens: list[QScreen] = QGuiApplication.screens()
        if 0 <= self.settings.monitor < len(screens):
            return screens[self.settings.monitor].geometry()
        geometry = screens[0].geometry()
        for screen in screens[1:]:
            geometry = geometry.united(screen.geometry())
        return geometry

    def _apply_win32_styles(self) -> None:
        user32 = ctypes.windll.user32
        hwnd = int(self.winId())
        ex_style = user32.GetWindowLongW(hwnd, GWL_EXSTYLE)
        ex_style |= WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
        user32.SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style)
        # Re-apply the styles: NOSIZE|NOMOVE|NOZORDER|NOACTIVATE|SHOWWINDOW.
        user32.SetWindowPos(hwnd, 0xFFFFFFFF, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0004 | 0x0010 | 0x0020)

    def _register_hotkey(self) -> None:
        self._toggle_hotkey_id = TOGGLE_HOTKEY_ID
        ctypes.windll.user32.RegisterHotKey(
            int(self.winId()), self._toggle_hotkey_id, MOD_CONTROL | MOD_SHIFT, VK_H
        )

    # ------------------------------------------------------------- logic --
    def _poll(self) -> None:
        """Track the pointer and repaint only when it moved."""
        pos = QCursor.pos()
        if pos != self.last_pos:
            self.last_pos = pos
            self.update()

    def toggle_native_cursor(self) -> None:
        """Show/hide the native Windows cursor (Ctrl+Shift+H)."""
        self.native_cursor_hidden = not self.native_cursor_hidden
        self.setCursor(
            Qt.CursorShape.BlankCursor
            if self.native_cursor_hidden
            else Qt.CursorShape.ArrowCursor
        )

    # ------------------------------------------------------------- events --
    def paintEvent(self, event):  # noqa: N802 (Qt naming convention)
        if self.last_pos is None:
            return
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        center = QPointF(self.mapFromGlobal(self.last_pos))
        draw_cursor(
            painter,
            center,
            self.settings.size,
            self.color,
            self.settings.thickness,
            self.settings.style,
            self.settings.gap,
        )

    def nativeEvent(self, event_type, message):  # noqa: N802
        """Handle the WM_HOTKEY message for the global toggle hotkey."""
        msg = ctypes.wintypes.MSG.from_address(int(message))
        if msg.message == WM_HOTKEY and msg.wParam == self._toggle_hotkey_id:
            self.toggle_native_cursor()
            return True, 0
        return super().nativeEvent(event_type, message)
