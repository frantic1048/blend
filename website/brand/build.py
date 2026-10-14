"""Generate Blend's identity assets.

Run from the website directory:

    uv run --with fonttools --with brotli --with uharfbuzz python brand/build.py

Text is shaped with HarfBuzz from the site's own variable fonts and written as
outlines, so every SVG is self-contained. PNG rasters require `rsvg-convert`
(librsvg). The spoon is the wand-like spoon from the Pyon Pyon Today logo.
"""

import io
import math
import subprocess
import tempfile
from pathlib import Path

import uharfbuzz as hb
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

WEBSITE = Path(__file__).resolve().parent.parent
FONTS = WEBSITE / "public" / "fonts"
PUBLIC = WEBSITE / "public"

ESPRESSO = "#2b1d16"
INK_SOFT = "#6a5345"
ROAST = "#7a4a2c"
CREAM = "#f8f1e6"
PAPER = "#fffaf2"
RULE = "#d9c4a7"

# The spoon from pyonpyon.today/image/logo.svg, translated to its bounding box.
SPOON = (Path(__file__).parent / "spoon.path").read_text().strip()
SPOON_W, SPOON_H = 19.76, 53.68
SPOON_PIVOT = (SPOON_W * 0.36, SPOON_H / 2)
# Two points on the handle's centre line, used to break the underline around it.
SPOON_HANDLE = ((6.9, 16.0), (2.5, 47.0))


def fmt(value: float) -> str:
    return f"{value:.2f}".rstrip("0").rstrip(".")


# Text -----------------------------------------------------------------------

_faces: dict[Path, tuple[hb.Face, TTFont]] = {}


def text_path(font: str, text: str, size: float, axes: dict, features=None, tracking=0.0):
    """Shape `text` and return (path data, advance) with the baseline at y=0."""
    path = FONTS / font
    if path not in _faces:
        tt = TTFont(path)
        tt.flavor = None
        buffer = io.BytesIO()
        tt.save(buffer)
        _faces[path] = (hb.Face(buffer.getvalue()), tt)
    face, tt = _faces[path]
    hb_font = hb.Font(face)
    hb_font.set_variations(axes)
    buf = hb.Buffer()
    buf.add_str(text)
    buf.guess_segment_properties()
    hb.shape(hb_font, buf, {"kern": True, "liga": True, **(features or {})})

    scale = size / face.upem
    glyphs = tt.getGlyphSet(location=axes)
    names = tt.getGlyphOrder()
    x, parts = 0.0, []
    for info, pos in zip(buf.glyph_infos, buf.glyph_positions):
        pen = SVGPathPen(glyphs, ntos=fmt)
        transform = (scale, 0, 0, -scale, x + pos.x_offset * scale, -pos.y_offset * scale)
        glyphs[names[info.codepoint]].draw(TransformPen(pen, transform))
        parts.append(pen.getCommands())
        x += pos.x_advance * scale + tracking * size
    return " ".join(p for p in parts if p), x - (tracking * size if text else 0)


def wordmark(size: float):
    return text_path("fraunces-italic-variable.woff2", "blend", size,
                     {"wght": 400, "opsz": 144, "SOFT": 0, "WONK": 0})


def smallcaps(text: str, size: float):
    return text_path("eb-garamond-variable.woff2", text, size, {"wght": 500},
                     {"smcp": True, "c2sc": True}, tracking=0.1)


# Lockup ---------------------------------------------------------------------


def spoon_transform(x: float, y: float, height: float, angle: float) -> str:
    s = height / SPOON_H
    px, py = SPOON_PIVOT
    return f"translate({fmt(x)} {fmt(y)}) rotate({fmt(angle)}) scale({s:.4f}) translate({-px} {-py})"


def spoon_point(point, x, y, height, angle):
    s = height / SPOON_H
    px, py = (point[0] - SPOON_PIVOT[0]) * s, (point[1] - SPOON_PIVOT[1]) * s
    a = math.radians(angle)
    return (x + px * math.cos(a) - py * math.sin(a), y + px * math.sin(a) + py * math.cos(a))


def lens(x0: float, x1: float, y: float, thickness: float, fill: str) -> str:
    """A hairline that swells slightly in the middle, like the Pyon Pyon rule."""
    m, t = (x0 + x1) / 2, thickness
    return (f'<path d="M{fmt(x0)} {fmt(y)}Q{fmt(m)} {fmt(y - t)} {fmt(x1)} {fmt(y)}'
            f'Q{fmt(m)} {fmt(y + t)} {fmt(x0)} {fmt(y)}Z" fill="{fill}"/>')


def lockup_defs(prefix: str) -> str:
    return f"""<defs>
<linearGradient id="{prefix}-spoon" x1="0" y1="0" x2="0" y2="1">
<stop offset="0" stop-color="#7499d0"/><stop offset=".5" stop-color="#8479bd"/><stop offset="1" stop-color="#c47d90"/>
</linearGradient>
<radialGradient id="{prefix}-blue"><stop offset="0" stop-color="#9dc2ea" stop-opacity=".85"/><stop offset=".6" stop-color="#9dc2ea" stop-opacity=".5"/><stop offset="1" stop-color="#9dc2ea" stop-opacity="0"/></radialGradient>
<radialGradient id="{prefix}-violet"><stop offset="0" stop-color="#ad9fdc" stop-opacity=".8"/><stop offset=".6" stop-color="#ad9fdc" stop-opacity=".45"/><stop offset="1" stop-color="#ad9fdc" stop-opacity="0"/></radialGradient>
<radialGradient id="{prefix}-rose"><stop offset="0" stop-color="#e9b0bd" stop-opacity=".8"/><stop offset=".6" stop-color="#e9b0bd" stop-opacity=".45"/><stop offset="1" stop-color="#e9b0bd" stop-opacity="0"/></radialGradient>
<filter id="{prefix}-fog" x="-25%" y="-50%" width="150%" height="200%" color-interpolation-filters="sRGB">
<feGaussianBlur in="SourceGraphic" stdDeviation="6" result="soft"/>
<feTurbulence type="fractalNoise" baseFrequency=".035" numOctaves="5" seed="27" result="noise"/>
<feComposite in="noise" in2="soft" operator="over" result="mix"/>
<feColorMatrix in="mix" values="1 0 0 0 0 0 1 0 0 0 0 0 1 0 0 0 0 0 6 -4" result="mask"/>
<feDisplacementMap in="soft" in2="noise" scale="30" xChannelSelector="A" yChannelSelector="A" result="moved"/>
<feComposite in="moved" in2="mask" operator="in" result="cloud"/>
<feComposite in="cloud" in2="soft" operator="in"/>
</filter>
<filter id="{prefix}-glow" x="-40%" y="-15%" width="180%" height="130%" color-interpolation-filters="sRGB">
<feGaussianBlur in="SourceGraphic" stdDeviation="1.4" result="blur"/>
<feComposite in="blur" in2="SourceGraphic" operator="in"/>
</filter>
</defs>"""


def lockup(ink: str, *, color: bool = True, prefix: str = "blend", bleed: bool = True):
    """Return (svg body, (x, y, width, height)) for the wordmark lockup.

    The wordmark sits on a fine rule; the spoon stirs through the rule just past
    the final letter, breaking it where the handle passes. In colour, a fog of
    blue, violet, and rose sits behind everything. The fog's base shapes are
    soft radial gradients, so a renderer without filter support still draws a
    gentle glow instead of hard edges, and the letterforms never depend on it.
    """
    size = 100
    d, advance = wordmark(size)
    rule_y, rule_t = 20, 1.5
    sx, sy, sh, angle = advance + 26, -22, 128, 18
    top = spoon_point(SPOON_HANDLE[0], sx, sy, sh, angle)
    bottom = spoon_point(SPOON_HANDLE[1], sx, sy, sh, angle)
    cross = top[0] + (rule_y - top[1]) * (bottom[0] - top[0]) / (bottom[1] - top[1])
    gap = 4.5
    x0, x1 = -14, advance + 58

    parts = []
    if color:
        parts.append(lockup_defs(prefix))
        cx, w, h = advance * 0.58, advance * 1.1, 128
        parts.append(
            f'<g class="blend-fog" filter="url(#{prefix}-fog)" opacity=".85">'
            f'<ellipse cx="{fmt(cx - w * .3)}" cy="2" rx="{fmt(w * .32)}" ry="{fmt(h * .48)}" fill="url(#{prefix}-blue)"/>'
            f'<ellipse cx="{fmt(cx + w * .02)}" cy="10" rx="{fmt(w * .3)}" ry="{fmt(h * .44)}" fill="url(#{prefix}-violet)"/>'
            f'<ellipse cx="{fmt(cx + w * .36)}" cy="-14" rx="{fmt(w * .32)}" ry="{fmt(h * .56)}" fill="url(#{prefix}-rose)"/>'
            f"</g>")
    parts.append(f'<path d="{d}" fill="{ink}"/>')
    parts.append(lens(x0, cross - gap, rule_y, rule_t, ink))
    parts.append(lens(cross + gap, x1, rule_y, rule_t * 0.6, ink))
    spoon_fill = f"url(#{prefix}-spoon)" if color else ink
    glow = f' filter="url(#{prefix}-glow)"' if color else ""
    parts.append(f'<path class="blend-spoon" transform="{spoon_transform(sx, sy, sh, angle)}" d="{SPOON}" fill="{spoon_fill}"{glow}/>')

    corners = [spoon_point(c, sx, sy, sh, angle)
               for c in ((0, 0), (SPOON_W, 0), (0, SPOON_H), (SPOON_W, SPOON_H))]
    pad = 26 if color and bleed else 6
    left, top = x0 - pad, min([-size * 0.82] + [c[1] for c in corners]) - pad
    right, bottom = max([x1] + [c[0] for c in corners]) + pad, max(c[1] for c in corners) + pad
    return "".join(parts), (left, top, right - left, bottom - top)


def svg(box, body: str, extra: str = "") -> str:
    x, y, w, h = (fmt(v) for v in box)
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{x} {y} {w} {h}"{extra}>'
            f"{body}</svg>\n")


# Compact mark ----------------------------------------------------------------

# A simplified spoon for small sizes, drawn upright around the origin: a full
# bowl, a tapering handle ending in a knob, and one wing from the right side of
# the neck. No gradients, blur, or fine detail.
MARK_BOWL = '<ellipse cx="0" cy="-15" rx="8.6" ry="12"/>'
MARK_HANDLE = '<path d="M-2.8 -4.5L2.8 -4.5L1.7 23L-1.7 23Z"/><circle cx="0" cy="25" r="3.2"/>'
MARK_WING = ('<path d="M1.6 -5.6C5 -10.4 11.4 -15.6 21.8 -15.4C21 -12.8 19.4 -11.2 17.2 -10.6'
             'C17.4 -8.2 15 -6.6 12.4 -7.2C11.2 -4.8 8.2 -4.2 6.2 -5.2C4.8 -2.8 3.4 0.2 2.4 3.8Z"/>')


def mark(fill_bowl: str, fill_wing: str, angle: float = 34) -> str:
    return (f'<g transform="rotate({angle})">'
            f'<g fill="{fill_bowl}">{MARK_BOWL}{MARK_HANDLE}</g>'
            f'<g fill="{fill_wing}">{MARK_WING}</g></g>')


def favicon() -> str:
    body = (f'<rect x="-32" y="-32" width="64" height="64" rx="14" fill="{ESPRESSO}"/>'
            f'<g transform="translate(-3 1) scale(1.06)">{mark("#bccdf0", "#ecbfcb")}</g>')
    return svg((-32, -32, 64, 64), body)


def mono_mark() -> str:
    body = f'<g transform="translate(-3 1) scale(1.06)">{mark("#000", "#000")}</g>'
    return svg((-32, -32, 64, 64), body)


# Social cards ----------------------------------------------------------------


def social_card(kicker: str, title: list[list[tuple[str, bool]]], subtitle: list[str]) -> str:
    """A 1200×630 card: headline on the left, the lockup on the right."""
    column = 560
    parts = [f'<rect width="1200" height="630" fill="{CREAM}"/>',
             f'<rect x="28" y="28" width="1144" height="574" fill="none" stroke="{RULE}" stroke-width="1.5"/>',
             f'<rect x="36" y="36" width="1128" height="558" fill="none" stroke="{RULE}" stroke-width="1"/>']

    body, box = lockup(ESPRESSO, prefix="card")
    scale = 450 / box[2]
    lx = 1150 - box[2] * scale - box[0] * scale
    ly = 315 - (box[1] + box[3] / 2) * scale
    parts.append(f'<g transform="translate({fmt(lx)} {fmt(ly)}) scale({scale:.4f})">{body}</g>')

    size, leading = 64, 70
    height = 30 + 42 + leading * (len(title) - 1) + 30 + 40 * len(subtitle)
    y = 315 - height / 2 + 22
    k, _ = smallcaps(kicker, 28)
    parts.append(f'<path transform="translate(100 {fmt(y)})" d="{k}" fill="{ROAST}"/>')
    y += 72
    for line in title:
        x = 96
        for text, italic in line:
            font = "fraunces-italic-variable.woff2" if italic else "fraunces-variable.woff2"
            axes = {"wght": 460 if italic else 540, "opsz": 72, "SOFT": 0, "WONK": 0}
            d, advance = text_path(font, text, size, axes)
            parts.append(f'<path transform="translate({fmt(x)} {fmt(y)})" d="{d}" fill="{ESPRESSO}"/>')
            x += advance
        assert x - 96 <= column, f"title line too wide: {line}"
        y += leading
    y += 4
    for line in subtitle:
        s, advance = text_path("eb-garamond-variable.woff2", line, 32, {"wght": 400})
        assert advance <= column, f"subtitle line too wide: {line}"
        parts.append(f'<path transform="translate(100 {fmt(y)})" d="{s}" fill="{INK_SOFT}"/>')
        y += 40
    u, _ = smallcaps("blend.pyonpyon.today", 24)
    parts.append(f'<path transform="translate(100 556)" d="{u}" fill="{INK_SOFT}"/>')
    return svg((0, 0, 1200, 630), "".join(parts), ' width="1200" height="630"')


# Output ----------------------------------------------------------------------


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)
    if path.is_relative_to(WEBSITE):
        print(f"wrote {path.relative_to(WEBSITE)}")


def rasterize(source: Path, target: Path, width: int) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(["rsvg-convert", "-w", str(width), "-o", str(target), str(source)], check=True)
    print(f"wrote {target.relative_to(WEBSITE)}")


def main() -> None:
    brand = PUBLIC / "brand"

    body, box = lockup(ESPRESSO)
    write(brand / "blend-lockup.svg", svg(box, f"<title>Blend</title>{body}"))
    body, box = lockup("#000", color=False)
    write(brand / "blend-lockup-mono.svg", svg(box, f"<title>Blend</title>{body}"))
    write(brand / "blend-mark-mono.svg", mono_mark())
    write(PUBLIC / "favicon.svg", favicon())

    # Inline masthead variant: the ink follows `currentColor`, so forced-colors
    # mode and link states recolour the letterforms while the spoon stays crisp.
    # The box is tight so the letterforms align with the page; the fog overflows.
    body, box = lockup("currentColor", prefix="masthead", bleed=False)
    write(WEBSITE / "src" / "brand" / "masthead.svg",
          svg(box, body, ' overflow="visible" aria-hidden="true" focusable="false"'))

    sources = Path(tempfile.mkdtemp(prefix="blend-brand-"))
    write(sources / "og-home.svg", social_card(
        "A dotfile manager for macOS & Linux",
        [[("Declarative dotfiles,", False)], [("thoughtfully", True)], [("reconciled.", False)]],
        ["Compose in Nickel, compare by meaning,", "and reconcile in both directions."]))
    write(sources / "og-guide.svg", social_card(
        "Blend documentation",
        [[("The ", False), ("user guide", True)]],
        ["Writing Orders, composing in Nickel,", "formats, and safe reconciliation."]))
    write(sources / "apple-touch-icon.svg", favicon().replace('rx="14"', 'rx="0"'))

    rasterize(sources / "og-home.svg", PUBLIC / "og" / "home.png", 1200)
    rasterize(sources / "og-guide.svg", PUBLIC / "og" / "guide.png", 1200)
    rasterize(sources / "apple-touch-icon.svg", PUBLIC / "apple-touch-icon.png", 180)


if __name__ == "__main__":
    main()
