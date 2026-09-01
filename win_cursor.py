"""Win32 helpers to hide/show the system and Windows Ink pen cursors.

These mechanisms are *global*, so they work even though the overlay window
itself is click-through (WS_EX_TRANSPARENT) and therefore never receives
WM_SETCURSOR / WM_POINTER* messages:

* Classic mouse cursor : ShowCursor(FALSE) hides it for the whole desktop.
* Windows Ink pen cursor: the WISP pen hover cursor is controlled by the
  HKCU\\Control Panel\\Cursors\\PenVisualization DWORD (default 0x23).
  Setting it to 0 disables the pen cursor everywhere -- the same setting the
  Windows UI exposes as "Pen & Windows Ink -> Show cursor".
"""

import ctypes
import ctypes.wintypes
import winreg

# --- cursor state ----------------------------------------------------------
class CURSORINFO(ctypes.Structure):
    """CURSORINFO structure used by GetCursorInfo."""

    _fields_ = [
        ("cbSize", ctypes.wintypes.DWORD),
        ("flags", ctypes.wintypes.DWORD),
        ("hCursor", ctypes.wintypes.HANDLE),
        ("ptScreenPos", ctypes.wintypes.POINT),
    ]


CURSOR_SHOWING = 0x00000001

# --- pen cursor registry value --------------------------------------------
PEN_VIS_KEY = r"Control Panel\Cursors"
PEN_VIS_NAME = "PenVisualization"
PEN_VIS_DEFAULT = 0x23  # default bitmask: pen cursor visual is shown
PEN_VIS_HIDDEN = 0x00   # disables the pen cursor visual

# --- misc messages / flags -------------------------------------------------
WM_SETTINGCHANGE = 0x001A
HWND_BROADCAST = 0xFFFF
SMTO_ABORTIFHUNG = 0x0002


def _user32():
    return ctypes.windll.user32


def cursor_is_showing() -> bool:
    """Return True if the classic cursor is currently displayed anywhere."""
    info = CURSORINFO()
    info.cbSize = ctypes.sizeof(CURSORINFO)
    if not _user32().GetCursorInfo(ctypes.byref(info)):
        return False
    return bool(info.flags & CURSOR_SHOWING)


def hide_cursor() -> None:
    """Hide the classic cursor for the whole desktop (idempotent)."""
    user32 = _user32()
    for _ in range(64):
        if not cursor_is_showing():
            return
        user32.ShowCursor(False)


def show_cursor() -> None:
    """Show the classic cursor again regardless of the hide count."""
    user32 = _user32()
    for _ in range(64):
        if cursor_is_showing():
            return
        user32.ShowCursor(True)


def set_pen_cursor_visualization(enabled: bool):
    """Enable/disable the Windows Ink pen cursor via the registry.

    Returns the previous PenVisualization value (so it can be restored on
    exit), or None if the registry value could not be changed.
    """
    key = None
    try:
        try:
            key = winreg.OpenKey(
                winreg.HKEY_CURRENT_USER, PEN_VIS_KEY, 0,
                winreg.KEY_READ | winreg.KEY_WRITE,
            )
        except FileNotFoundError:
            key = winreg.CreateKey(winreg.HKEY_CURRENT_USER, PEN_VIS_KEY)

        try:
            previous, _ = winreg.QueryValueEx(key, PEN_VIS_NAME)
        except FileNotFoundError:
            previous = PEN_VIS_DEFAULT

        winreg.SetValueEx(
            key, PEN_VIS_NAME, 0, winreg.REG_DWORD,
            PEN_VIS_DEFAULT if enabled else PEN_VIS_HIDDEN,
        )
        _broadcast_setting_change()
        return int(previous)
    except OSError:
        return None
    finally:
        if key is not None:
            winreg.CloseKey(key)


def restore_pen_cursor_visualization(value: int) -> None:
    """Restore a previously saved PenVisualization value."""
    try:
        with winreg.OpenKey(
            winreg.HKEY_CURRENT_USER, PEN_VIS_KEY, 0, winreg.KEY_WRITE
        ) as key:
            winreg.SetValueEx(key, PEN_VIS_NAME, 0, winreg.REG_DWORD, int(value))
        _broadcast_setting_change()
    except OSError:
        pass


def _broadcast_setting_change() -> None:
    """Tell the shell a user setting changed so it re-reads the value.

    Best-effort only: never raises -- the registry value is the source of
    truth, so a broadcast failure must not break cursor hiding.
    """
    try:
        user32 = _user32()
        user32.SendMessageTimeoutW.argtypes = (
            ctypes.c_void_p, ctypes.c_uint, ctypes.c_size_t, ctypes.c_void_p,
            ctypes.c_uint, ctypes.c_uint, ctypes.POINTER(ctypes.c_size_t),
        )
        user32.SendMessageTimeoutW.restype = ctypes.c_ssize_t
        payload = ctypes.create_unicode_buffer("Cursors")
        # cast() requires a pointer type as its second argument.
        user32.SendMessageTimeoutW(
            HWND_BROADCAST, WM_SETTINGCHANGE, 0,
            ctypes.cast(payload, ctypes.c_void_p),
            SMTO_ABORTIFHUNG, 200, None,
        )
    except Exception:
        pass
