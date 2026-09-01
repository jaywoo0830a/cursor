"""Command-line settings for the custom cursor overlay.

All visual knobs (style, size, color, ...) can be tuned from the CLI so the
overlay can be adjusted per drawing app without touching code.
"""

import argparse

CURSOR_STYLES = ("crosshair", "ring", "dot", "ring_cross", "cross_dot")


def parse_args(argv=None) -> argparse.Namespace:
    """Build and parse the command-line arguments."""
    parser = argparse.ArgumentParser(
        description="Windows custom mouse cursor overlay (great with a drawing pad).",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    parser.add_argument(
        "--style",
        default="ring_cross",
        choices=CURSOR_STYLES,
        help="Cursor shape to draw",
    )
    parser.add_argument(
        "--size",
        type=int,
        default=40,
        help="Cursor diameter in logical pixels",
    )
    parser.add_argument(
        "--color",
        default="#FF2D55",
        help="Cursor color as hex, e.g. #FF2D55",
    )
    parser.add_argument(
        "--gap",
        type=float,
        default=0.35,
        help="Crosshair gap around the exact pointer position (fraction of the radius)",
    )
    parser.add_argument(
        "--thickness",
        type=int,
        default=2,
        help="Stroke width in pixels",
    )
    parser.add_argument(
        "--fps",
        type=int,
        default=200,
        help="Overlay refresh rate in frames per second (200 = smooth on "
        "high-refresh monitors)",
    )
    parser.add_argument(
        "--monitor",
        type=int,
        default=-1,
        help="0-based monitor index to show the cursor on; -1 = all monitors",
    )
    parser.add_argument(
        "--hide-system-cursor",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Hide the native Windows cursor (mouse + pen) while the overlay "
        "runs. Use --no-hide-system-cursor to keep it visible.",
    )
    parser.add_argument(
        "--show-system-cursor",
        action="store_true",
        help=argparse.SUPPRESS,  # legacy alias for --no-hide-system-cursor
    )
    parser.add_argument(
        "--no-hide-pen-cursor",
        action="store_true",
        help="Do not disable the Windows Ink pen cursor (registry value "
        "HKCU\\Control Panel\\Cursors\\PenVisualization) while the overlay runs",
    )
    parser.add_argument(
        "--click-through",
        action="store_true",
        help="Keep the overlay click-through (classic mode). In this mode the "
        "system cursor cannot be hidden over other applications' windows.",
    )
    args = parser.parse_args(argv)

    # The native cursor is hidden by default; --show-system-cursor overrides it.
    if args.show_system_cursor:
        args.hide_system_cursor = False
    args.hide_pen_cursor = not args.no_hide_pen_cursor
    return args
