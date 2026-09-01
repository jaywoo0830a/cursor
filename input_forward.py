"""Helpers for the overlay's default "own the cursor" mode.

The overlay is the top-most, hit-testable, full-screen window, so it owns the
system cursor and can hide it globally (mouse + pen) via WM_SETCURSOR.  To
keep the application below usable, the overlay becomes click-through
(WS_EX_TRANSPARENT) the instant a button is pressed and re-injects that press
as real input; once every button is released it returns to cursor-owner mode.

Windows-only.
"""

import ctypes
import ctypes.wintypes

# --- message ids ------------------------------------------------------------
WM_MOUSEACTIVATE = 0x0021
WM_MOUSEWHEEL = 0x020A
WM_LBUTTONDOWN = 0x0201
WM_RBUTTONDOWN = 0x0204
WM_MBUTTONDOWN = 0x0207
WM_XBUTTONDOWN = 0x020B
WM_POINTERDOWN = 0x0246

MA_NOACTIVATE = 0x0003

# --- SendInput --------------------------------------------------------------
INPUT_MOUSE = 0
MOUSEEVENTF_LEFTDOWN = 0x0002
MOUSEEVENTF_LEFTUP = 0x0004
MOUSEEVENTF_RIGHTDOWN = 0x0008
MOUSEEVENTF_RIGHTUP = 0x0010
MOUSEEVENTF_MIDDLEDOWN = 0x0020
MOUSEEVENTF_MIDDLEUP = 0x0040
MOUSEEVENTF_XDOWN = 0x0080
MOUSEEVENTF_XUP = 0x0100

# Which mouse-button "down" message maps to which injected event.
DOWN_TO_FLAGS = {
    WM_LBUTTONDOWN: MOUSEEVENTF_LEFTDOWN,
    WM_RBUTTONDOWN: MOUSEEVENTF_RIGHTDOWN,
    WM_MBUTTONDOWN: MOUSEEVENTF_MIDDLEDOWN,
    WM_XBUTTONDOWN: MOUSEEVENTF_XDOWN,
}

# --- GetAsyncKeyState -------------------------------------------------------
VK_LBUTTON = 0x01
VK_RBUTTON = 0x02
VK_MBUTTON = 0x04
VK_XBUTTON1 = 0x05
VK_XBUTTON2 = 0x06

# --- structures -------------------------------------------------------------
class MOUSEINPUT(ctypes.Structure):
    _fields_ = [
        ("dx", ctypes.wintypes.LONG),
        ("dy", ctypes.wintypes.LONG),
        ("mouseData", ctypes.wintypes.DWORD),
        ("dwFlags", ctypes.wintypes.DWORD),
        ("time", ctypes.wintypes.DWORD),
        ("dwExtraInfo", ctypes.c_ulong),
    ]


class INPUT(ctypes.Structure):
    class _U(ctypes.Union):
        _fields_ = [("mi", MOUSEINPUT)]

    _anonymous_ = ("u",)
    _fields_ = [("type", ctypes.wintypes.DWORD), ("u", _U)]


def _user32():
    return ctypes.windll.user32


def inject_button(dw_flags: int) -> None:
    """Inject a synthetic mouse button event at the current cursor position."""
    inp = INPUT()
    inp.type = INPUT_MOUSE
    inp.mi.dwFlags = dw_flags
    _user32().SendInput(1, ctypes.byref(inp), ctypes.sizeof(INPUT))


def buttons_pressed() -> bool:
    """True while any mouse button (including pen contact) is held down."""
    user32 = _user32()
    return any(
        user32.GetAsyncKeyState(vk) & 0x8000
        for vk in (VK_LBUTTON, VK_RBUTTON, VK_MBUTTON, VK_XBUTTON1, VK_XBUTTON2)
    )


def window_below(exclude_hwnd, x: int, y: int):
    """Top-most visible top-level window at (x, y) other than exclude_hwnd."""
    user32 = _user32()
    hwnd = user32.GetWindow(user32.GetDesktopWindow(), 5)  # GW_CHILD
    while hwnd:
        if hwnd != exclude_hwnd and not user32.IsChild(exclude_hwnd, hwnd):
            if user32.IsWindowVisible(hwnd):
                rect = ctypes.wintypes.RECT()
                user32.GetWindowRect(hwnd, ctypes.byref(rect))
                if rect.left <= x < rect.right and rect.top <= y < rect.bottom:
                    return hwnd
        hwnd = user32.GetWindow(hwnd, 2)  # GW_HWNDNEXT
    return None


def forward_mouse_message(msg: int, hwnd_target, x: int, y: int, wparam) -> None:
    """Send a mouse message to hwnd_target using client coordinates."""
    if not hwnd_target:
        return
    user32 = _user32()
    pt = ctypes.wintypes.POINT(x, y)
    user32.ScreenToClient(hwnd_target, ctypes.byref(pt))
    lparam = ((pt.y & 0xFFFF) << 16) | (pt.x & 0xFFFF)
    user32.SendMessageW(hwnd_target, msg, wparam, lparam)
