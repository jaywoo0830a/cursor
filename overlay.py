"""Full-screen overlay that paints a custom cursor and hides the system one.

The overlay covers the whole (virtual) desktop and draws a highly visible
custom cursor (ring / crosshair / dot) at the exact pointer position.  This is
especially useful with a drawing pad, where precise pen-position feedback
matters.

How the system cursor is hidden (default "own the cursor" mode):

* The overlay is the top-most, hit-testable, full-screen window, so it owns
  the system cursor.  WM_SETCURSOR is answered with SetCursor(NULL) + TRUE,
  which hides the cursor (mouse AND Windows Ink pen) everywhere, no matter
  which application window is underneath -- ShowCursor alone is thread-local
  and only works over windows owned by our thread.
* To keep the application below usable, the overlay becomes click-through
  (WS_EX_TRANSPARENT) the instant a button is pressed and re-injects that
  press as real input, so the app still receives the click and the full pen
  stroke (including pressure).  When every button is released it becomes the
  cursor owner again.
* The "PenVisualization" HKCU registry value is also set to 0 as extra
  insurance for the Windows Ink pen cursor (restored on exit).

Pass --click-through to keep the classic click-through behavior instead.

Windows-only (uses Win32 APIs via ctypes).
"""

import ctypes
import ctypes.wintypes

from PySide6.QtCore import QPointF, QRect, Qt, QTimer
from PySide6.QtGui import QColor, QCursor, QGuiApplication, QPainter, QScreen
from PySide6.QtWidgets import QWidget

import input_forward
import win_cursor
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
WM_SETCURSOR = 0x0020
TOGGLE_HOTKEY_ID = 1


class CursorOverlay(QWidget):
    """Translucent, always-on-top window that tracks and redraws the cursor."""

    def __init__(self, settings):
        super().__init__(None)
        self.settings = settings
        self.color = QColor(settings.color)
        self.last_pos = None
        self.native_cursor_hidden = settings.hide_system_cursor
        self._pen_vis_prev = None       # previous PenVisualization registry value
        self._transient_click_through = False  # stroke pass-through in progress

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
        self.setAttribute(Qt.WidgetAttribute.WA_ShowWithoutActivating)
        self.setGeometry(self._virtual_geometry())

        # Guard timer: restores cursor ownership after a click/stroke and
        # re-hides the cursor if Windows re-asserts it.  The actual cursor
        # policy is applied in finalize() once the native HWND exists.
        self._cursor_guard = QTimer(self)
        self._cursor_guard.setInterval(50)
        self._cursor_guard.timeout.connect(self._guard_cursor)

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
        self._apply_cursor_policy()

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
        ex_style |= WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
        user32.SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style)
        # Re-apply the styles: NOSIZE|NOMOVE|NOZORDER|NOACTIVATE|SHOWWINDOW.
        user32.SetWindowPos(hwnd, 0xFFFFFFFF, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0004 | 0x0010 | 0x0020)

    def _register_hotkey(self) -> None:
        self._toggle_hotkey_id = TOGGLE_HOTKEY_ID
        ctypes.windll.user32.RegisterHotKey(
            int(self.winId()), self._toggle_hotkey_id, MOD_CONTROL | MOD_SHIFT, VK_H
        )

    # ----------------------------------------------------- cursor hiding --
    def _set_click_through(self, enabled: bool) -> None:
        """Dynamically toggle WS_EX_TRANSPARENT on the overlay window."""
        user32 = ctypes.windll.user32
        hwnd = int(self.winId())
        ex_style = user32.GetWindowLongW(hwnd, GWL_EXSTYLE)
        if enabled:
            ex_style |= WS_EX_TRANSPARENT
        else:
            ex_style &= ~WS_EX_TRANSPARENT
        user32.SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style)

    def _apply_cursor_policy(self) -> None:
        """Apply the hide/show policy for the native + pen cursors.

        Default mode: the overlay owns the cursor (hit-testable) so it can
        hide the system cursor globally.  --click-through keeps the classic
        transparent behavior instead.
        """
        own_cursor = self.native_cursor_hidden and not self.settings.click_through
        if own_cursor:
            if self._transient_click_through:
                self._transient_click_through = False
                self._set_click_through(False)
            win_cursor.hide_cursor()
            if self.settings.hide_pen_cursor and self._pen_vis_prev is None:
                self._pen_vis_prev = win_cursor.set_pen_cursor_visualization(False)
            self.setCursor(Qt.CursorShape.BlankCursor)
        else:
            self._set_click_through(True)
            win_cursor.show_cursor()
            if self._pen_vis_prev is not None:
                win_cursor.restore_pen_cursor_visualization(self._pen_vis_prev)
                self._pen_vis_prev = None
            self.setCursor(Qt.CursorShape.ArrowCursor)
        self._cursor_guard.start()

    def _begin_stroke(self, down_msg: int) -> None:
        """Hand a press to the app below: click-through + re-inject it."""
        if self._transient_click_through:
            return
        self._transient_click_through = True
        self._set_click_through(True)
        flags = input_forward.DOWN_TO_FLAGS.get(down_msg)
        if flags is not None:
            input_forward.inject_button(flags)

    def _guard_cursor(self) -> None:
        """Restore cursor ownership after a stroke; re-hide if needed."""
        if self._transient_click_through:
            if not input_forward.buttons_pressed():
                self._transient_click_through = False
                self._set_click_through(False)
            return
        if self.native_cursor_hidden and win_cursor.cursor_is_showing():
            win_cursor.hide_cursor()

    def restore_system_cursor(self) -> None:
        """Restore the native + pen cursors (call when the app exits)."""
        self.native_cursor_hidden = False
        self._transient_click_through = False
        self._set_click_through(True)
        win_cursor.show_cursor()
        if self._pen_vis_prev is not None:
            win_cursor.restore_pen_cursor_visualization(self._pen_vis_prev)
            self._pen_vis_prev = None
        self.setCursor(Qt.CursorShape.ArrowCursor)
        self._cursor_guard.stop()

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
        self._apply_cursor_policy()

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
        """Handle hotkey, global cursor hiding and input pass-through."""
        msg = ctypes.wintypes.MSG.from_address(int(message))
        if msg.message == WM_HOTKEY and msg.wParam == self._toggle_hotkey_id:
            self.toggle_native_cursor()
            return True, 0

        # Only the default "own the cursor" mode needs the input handling;
        # in click-through mode just pass everything through to Qt.
        if self.settings.click_through or not self.native_cursor_hidden:
            return super().nativeEvent(event_type, message)

        # We own the system cursor: keep it hidden on every move.
        if msg.message == WM_SETCURSOR:
            ctypes.windll.user32.SetCursor(None)
            return True, 0
        # Never activate the overlay itself.
        if msg.message == input_forward.WM_MOUSEACTIVATE:
            return True, input_forward.MA_NOACTIVATE
        # A press (mouse or pen) starts a stroke: become click-through so the
        # app below receives the real input (pen pressure included).
        if msg.message == input_forward.WM_POINTERDOWN:
            self._begin_stroke(input_forward.WM_LBUTTONDOWN)  # pen = left button
            return True, 0
        if msg.message in input_forward.DOWN_TO_FLAGS:
            self._begin_stroke(msg.message)
            return True, 0
        # Keep mouse wheel scrolling working over the overlay.
        if msg.message == input_forward.WM_MOUSEWHEEL:
            x = ctypes.c_short(msg.lParam & 0xFFFF).value
            y = ctypes.c_short((msg.lParam >> 16) & 0xFFFF).value
            target = input_forward.window_below(int(self.winId()), x, y)
            if target:
                input_forward.forward_mouse_message(
                    msg.message, target, x, y, msg.wParam
                )
            return True, 0

        return super().nativeEvent(event_type, message)
