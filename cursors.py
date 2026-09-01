"""Cursor shapes painted by the overlay.

Every shape is centered on the exact pointer position, so with the native
cursor hidden these shapes are the only cursor feedback on screen.
"""

from PySide6.QtCore import QPointF, QRectF, Qt
from PySide6.QtGui import QColor, QPainter, QPen


def draw_cursor(
    painter: QPainter,
    center: QPointF,
    size: int,
    color: QColor,
    thickness: int,
    style: str,
    gap_fraction: float = 0.35,
) -> None:
    """Draw the selected cursor shape around ``center``.

    Args:
        painter: Active QPainter (antialiasing expected).
        center: Exact pointer position in widget coordinates.
        size: Cursor diameter in logical pixels.
        color: Stroke color.
        thickness: Stroke width in pixels.
        style: One of CURSOR_STYLES in settings.py.
        gap_fraction: Crosshair gap as a fraction of the radius.
    """
    pen = QPen(color, thickness)
    pen.setCapStyle(Qt.PenCapStyle.RoundCap)
    pen.setJoinStyle(Qt.PenJoinStyle.RoundJoin)
    painter.setPen(pen)
    painter.setBrush(Qt.BrushStyle.NoBrush)

    radius = size / 2.0
    has_dot = style in ("dot", "cross_dot")
    gap = 0.0 if has_dot else radius * gap_fraction

    if style in ("crosshair", "ring_cross", "cross_dot"):
        _draw_cross(painter, center, radius, gap)
    if style in ("ring", "ring_cross"):
        _draw_ring(painter, center, radius)
    if has_dot:
        _draw_dot(painter, center, thickness, color)


def _draw_cross(painter: QPainter, center: QPointF, radius: float, gap: float) -> None:
    """Four crosshair arms leaving a gap around the exact pointer position."""
    x, y = center.x(), center.y()
    painter.drawLine(QPointF(x - radius, y), QPointF(x - gap, y))
    painter.drawLine(QPointF(x + gap, y), QPointF(x + radius, y))
    painter.drawLine(QPointF(x, y - radius), QPointF(x, y - gap))
    painter.drawLine(QPointF(x, y + gap), QPointF(x, y + radius))


def _draw_ring(painter: QPainter, center: QPointF, radius: float) -> None:
    """Thin circle around the pointer position."""
    painter.drawEllipse(
        QRectF(center.x() - radius, center.y() - radius, radius * 2, radius * 2)
    )


def _draw_dot(painter: QPainter, center: QPointF, thickness: int, color: QColor) -> None:
    """Filled dot at the exact pointer position."""
    diameter = max(thickness * 2, 5)
    painter.setBrush(color)
    painter.drawEllipse(
        QRectF(center.x() - diameter / 2, center.y() - diameter / 2, diameter, diameter)
    )
