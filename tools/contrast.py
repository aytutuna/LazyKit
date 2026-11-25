"""Closest shade of each Lazy icon color that reads on a light and on a dark surface.

Text needs 4.5:1 against the surface, controls 3:1. The hue and saturation stay as
drawn; only the lightness moves, and only as far as it has to.
"""
import colorsys
import sys

ICONS = {
    "Scan": "#2F6FED",
    "Print": "#0E9F7E",
    "PDF": "#E5484D",
    "Send": "#7C5CFF",
    "Collect": "#E8930C",
}
SURFACES = {"light": "#FFFFFF", "dark": "#1C1C1E"}
TEXT = 4.5
CONTROL = 3.0


def to_rgb(code):
    code = code.lstrip("#")
    return tuple(int(code[i:i + 2], 16) for i in (0, 2, 4))


def to_hex(rgb):
    return "#%02X%02X%02X" % rgb


def luminance(rgb):
    def channel(c):
        c /= 255
        return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4
    r, g, b = (channel(c) for c in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def ratio(a, b):
    la, lb = sorted((luminance(to_rgb(a)), luminance(to_rgb(b))), reverse=True)
    return (la + 0.05) / (lb + 0.05)


def closest(color, surface, need):
    """Walk the lightness away from the surface until the contrast is enough."""
    r, g, b = (c / 255 for c in to_rgb(color))
    h, l, s = colorsys.rgb_to_hls(r, g, b)
    step = -0.001 if luminance(to_rgb(surface)) > 0.5 else 0.001
    while 0 <= l <= 1:
        rgb = tuple(round(c * 255) for c in colorsys.hls_to_rgb(h, l, s))
        shade = to_hex(rgb)
        if ratio(shade, surface) >= need:
            return shade
        l += step
    raise ValueError(f"{color} cannot reach {need}:1 on {surface}")


def table():
    rows = []
    for app, color in ICONS.items():
        for name, surface in SURFACES.items():
            text = closest(color, surface, TEXT)
            control = closest(color, surface, CONTROL)
            rows.append((app, name, text, ratio(text, surface), control, ratio(control, surface)))
    return rows


def main():
    print(f"{'app':8} {'surface':6} {'text':8} {'ratio':>6} {'control':8} {'ratio':>6}")
    for app, name, text, rt, control, rc in table():
        print(f"{app:8} {name:6} {text:8} {rt:6.2f} {control:8} {rc:6.2f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
