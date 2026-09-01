"""Overlay that paints a custom cursor and hides the system cursor.

Two modes:

* Full-screen mode (default, no --zone): the overlay covers the whole
  desktop and hides the system cursor globally by owning it (see below).
* Write-zone mode (--zone X,Y,W,H): only a rectangular region is covered.
  While the pointer is inside the region the system/pen cursor is hidden and
  only the custom cursor is shown; outside the region everything is normal.
  Ctrl+Shift+Z toggles EDIT mode to move/resize the region.

How the cursor is hidden:

* ShowCursor/SetCursor are thread-local in Win32, so a click-through overlay
  can never hide the cursor over other applications' windows.  Instead the
  overlay is the top-most, hit-testable window, so it OWNS the system cursor.
  WM_SETCURSOR is answered with SetCursor(NULL) + TRUE, which hides the
  cursor (mouse AND Windows Ink pen) wherever the overlay is the window under
  the pointer.
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

from PySide6.QtCore import QPoint, QPointF, QRect, QSize, Qt, QTimer
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

# Global hotkeys (Ctrl+Shift+...).
MOD_CONTROL = 0x0002
MOD_SHIFT = 0x0004
VK_H = 0x48  # toggle cursor hiding
VK_Z = 0x5A  # toggle write-zone edit mode
WM_HOTKEY = 0x0312
WM_SETCURSOR = 0x0020
TOGGLE_HOTKEY_ID = 1
EDIT_HOTKEY_ID = 2

RESIZE_HANDLE = 16  # px, corner area used to resize the write zone
MIN_ZONE = 64       # px, minimum zone size


class CursorOverlay(QWidget):
    """Translucent, always-on-top window that tracks and redraws the cursor."""

    def __init__(self, settings):
        super().__init__(None)
        self.settings = settings
        self.color = QColor(settings.color)
        self.last_pos = None
        self.native_cursor_hidden = settings.hide_system_cursor
        self.zone_enabled = settings.zone is not None
        self.edit_mode = settings.edit and self.zone_enabled
        self._pen_vis_prev = None       # previous PenVisualization registry value
        self._transient_click_through = False  # stroke pass-through in progress
        self._drag = False              # dragging the zone (edit mode)
        self._resizing = False
        self._drag_global = QPoint()
        self._drag_geo = QRect()

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
        self.setMouseTracking(True)

        # Guard timer: restores cursor ownership after a click/stroke and
        # re-asserts the window styles.  The actual cursor policy is applied
        # in finalize() once the native HWND exists.
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
        """Write-zone rect, or the combined geometry of all monitors."""
        if self.zone_enabled:
            x, y, w, h = self.settings.zone
            return QRect(x, y, w, h)
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
        # Re-apply: NOSIZE|NOMOVE|NOZORDER|NOACTIVATE|SHOWWINDOW.
        user32.SetWindowPos(hwnd, 0xFFFFFFFF, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0004 | 0x0010 | 0x0020)

    def showEvent(self, event):  # noqa: N802
        super().showEvent(event)
        self._apply_win32_styles()
        self._reassert_ex_style()

    def _register_hotkey(self) -> None:
        user32 = ctypes.windll.user32
        self._toggle_hotkey_id = TOGGLE_HOTKEY_ID
        user32.RegisterHotKey(
            int(self.winId()), self._toggle_hotkey_id, MOD_CONTROL | MOD_SHIFT, VK_H
        )
        self._edit_hotkey_id = EDIT_HOTKEY_ID
        if self.zone_enabled:
            user32.RegisterHotKey(
                int(self.winId()), self._edit_hotkey_id, MOD_CONTROL | MOD_SHIFT, VK_Z
            )

    # ----------------------------------------------------- cursor hiding --
    def _owns_cursor(self) -> bool:
        """True when the overlay should own/hide the system cursor."""
        return (self.native_cursor_hidden
                and not self.settings.click_through
                and not self.edit_mode)

    def _reassert_ex_style(self) -> None:
        """Keep the Win32 ex-style consistent (Qt may reset it)."""
        user32 = ctypes.windll.user32
        hwnd = int(self.winId())
        base = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
        if self.edit_mode:
            want_transparent = False
        else:
            want_transparent = self._transient_click_through or not self._owns_cursor()
        target = base | (WS_EX_TRANSPARENT if want_transparent else 0)
        ex = user32.GetWindowLongW(hwnd, GWL_EXSTYLE)
        if ex != target:
            user32.SetWindowLongW(hwnd, GWL_EXSTYLE, target)

    def _apply_cursor_policy(self) -> None:
        """Apply the cursor/transparency policy for the current mode."""
        if self.edit_mode:
            self._transient_click_through = False
            win_cursor.show_cursor()
            self.setCursor(Qt.CursorShape.ArrowCursor)
        elif self._owns_cursor():
            self._transient_click_through = False
            win_cursor.hide_cursor()
            if self.settings.hide_pen_cursor and self._pen_vis_prev is None:
                self._pen_vis_prev = win_cursor.set_pen_cursor_visualization(False)
            self.setCursor(Qt.CursorShape.BlankCursor)
        else:
            self._transient_click_through = False
            win_cursor.show_cursor()
            if self._pen_vis_prev is not None:
                win_cursor.restore_pen_cursor_visualization(self._pen_vis_prev)
                self._pen_vis_prev = None
            self.setCursor(Qt.CursorShape.ArrowCursor)
        self._reassert_ex_style()
        self._cursor_guard.start()

    def _begin_stroke(self, down_msg: int) -> None:
        """Hand a press to the app below: click-through + re-inject it."""
        if self._transient_click_through:
            return
        self._transient_click_through = True
        self._reassert_ex_style()
        flags = input_forward.DOWN_TO_FLAGS.get(down_msg)
        if flags is not None:
            input_forward.inject_button(flags)

    def _guard_cursor(self) -> None:
        """Restore cursor ownership after a stroke; re-hide if needed."""
        self._reassert_ex_style()
        if self._transient_click_through:
            if not input_forward.buttons_pressed():
                self._transient_click_through = False
                self._reassert_ex_style()
            return
        if self._owns_cursor() and win_cursor.cursor_is_showing():
            win_cursor.hide_cursor()

    def restore_system_cursor(self) -> None:
        """Restore the native + pen cursors (call when the app exits)."""
        self.native_cursor_hidden = False
        self.edit_mode = False
        self._transient_click_through = False
        win_cursor.show_cursor()
        if self._pen_vis_prev is not None:
            win_cursor.restore_pen_cursor_visualization(self._pen_vis_prev)
            self._pen_vis_prev = None
        self.setCursor(Qt.CursorShape.ArrowCursor)
        self._reassert_ex_style()
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

    def toggle_edit_mode(self) -> None:
        """Toggle write-zone edit mode (Ctrl+Shift+Z) to move/resize."""
        if not self.zone_enabled:
            return
        self.edit_mode = not self.edit_mode
        self._apply_cursor_policy()
        self.update()

    # --------------------------------------------------- zone manipulation --
    def _near_resize_handle(self, local: QPoint) -> bool:
        return (local.x() >= self.width() - RESIZE_HANDLE
                and local.y() >= self.height() - RESIZE_HANDLE)

    def mousePressEvent(self, event):  # noqa: N802
        if not self.edit_mode:
            super().mousePressEvent(event)
            return
        if event.button() == Qt.MouseButton.LeftButton:
            self._drag = True
            self._resizing = self._near_resize_handle(event.position().toPoint())
            self._drag_global = event.globalPosition().toPoint()
            self._drag_geo = self.geometry()
            event.accept()

    def mouseMoveEvent(self, event):  # noqa: N802
        if self.edit_mode and self._drag:
            delta = event.globalPosition().toPoint() - self._drag_global
            geo = self._drag_geo
            if self._resizing:
                w = max(MIN_ZONE, geo.width() + delta.x())
                h = max(MIN_ZONE, geo.height() + delta.y())
                self.setGeometry(QRect(geo.topLeft(), QSize(w, h)))
            else:
                self.move(geo.topLeft() + delta)
            event.accept()
            return
        super().mouseMoveEvent(event)

    def mouseReleaseEvent(self, event):  # noqa: N802
        self._drag = False
        if self.edit_mode:
            event.accept()

    # ------------------------------------------------------------- events --
    def paintEvent(self, event):  # noqa: N802 (Qt naming convention)
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)

        if self.zone_enabled:
            # Draw the write-zone border so the user can see the region.
            border = QColor("#00B0FF" if self.edit_mode else self.color)
            painter.setPen(border)
            painter.drawRect(self.rect().adjusted(0, 0, -1, -1))

        if self.edit_mode or self.last_pos is None:
            return
        if self.zone_enabled:
            local = self.mapFromGlobal(self.last_pos)
            if not self.rect().contains(local):
                return  # pointer outside the write zone
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
        """Handle hotkeys, cursor hiding and input pass-through."""
        msg = ctypes.wintypes.MSG.from_address(int(message))
        if self.settings.debug and msg.message != 0x0200:  # skip WM_MOUSEMOVE
            print(f"[debug] WM 0x{msg.message:04X} wParam={msg.wParam:#x} lParam={msg.lParam:#x}", flush=True)

        if msg.message == WM_HOTKEY:
            if msg.wParam == self._toggle_hotkey_id:
                self.toggle_native_cursor()
                return True, 0
            if msg.wParam == self._edit_hotkey_id and self.zone_enabled:
                self.toggle_edit_mode()
                return True, 0

        # Only the "own the cursor" modes need the input handling; in edit /
        # click-through / show-cursor modes just pass everything to Qt.
        if self.settings.click_through or not self.native_cursor_hidden or self.edit_mode:
            return super().nativeEvent(event_type, message)

        # We own the system cursor: keep it hidden on every move.
        if msg.message == WM_SETCURSOR:
            ctypes.windll.user32.SetCursor(None)
            return True, 0
        # Never activate the overlay itself.
        if msg.message == input_forward.WM_MOUSEACTIVATE:
            return True, input_forward.MA_NOACTIVATE
        # Claim pointer input so Windows does not draw the Windows Ink pen
        # hover cursor over our overlay (a window that handles WM_POINTER*
        # manages its own cursor).  Also track the pen position from it.
        if input_forward.WM_POINTERUPDATE <= msg.message <= input_forward.WM_POINTER_LAST:
            if msg.message == input_forward.WM_POINTERDOWN:
                self._begin_stroke(input_forward.WM_LBUTTONDOWN)  # pen = left button
            elif msg.message == input_forward.WM_POINTERUPDATE:
                x = ctypes.c_short(msg.lParam & 0xFFFF).value
                y = ctypes.c_short((msg.lParam >> 16) & 0xFFFF).value
                self.last_pos = QPoint(x, y)
                self.update()
            ctypes.windll.user32.SetCursor(None)
            return True, 0
        # Disable pen press-and-hold (right-click ring) inside the overlay.
        if msg.message == input_forward.WM_TABLET_QUERYSYSTEMGESTURESTATUS:
            return True, input_forward.TABLET_DISABLE_PRESSANDHOLD
        # A mouse-button press starts a stroke: become click-through so the
        # app below receives the real input.
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
