#!/usr/bin/env python3
"""
Render Phoretic markdown documents to PDF using ReportLab.

Usage:
    python render.py                    # render all .md files in src/ to target/
    python render.py whitepaper         # render src/strategy_whitepaper.md
    python render.py overview roadmap   # render specific documents
    python render.py src/foo.md         # render by path
"""
import copy
import re
import sys
from pathlib import Path
from reportlab.lib.pagesizes import letter
from reportlab.lib.styles import getSampleStyleSheet, ParagraphStyle
from reportlab.lib.units import inch
from reportlab.lib import colors
from reportlab.lib.enums import TA_LEFT, TA_CENTER, TA_JUSTIFY, TA_RIGHT
from reportlab.platypus import (
    SimpleDocTemplate, Paragraph, Spacer, HRFlowable,
    Table, TableStyle, PageBreak, KeepTogether,
    BaseDocTemplate, Frame, PageTemplate,
)
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont as RLTTFont

SCRIPT_DIR = Path(__file__).resolve().parent
# Resolved against the CALLING project's directory, not the script's, since
# render.py now lives in the shared _meta/docs-render and is invoked from each
# project's docs/ directory. Overridable with --out-dir.
PROJECT_DIR = Path.cwd()
SRC_DIR = PROJECT_DIR / "src"
TARGET_DIR = PROJECT_DIR / "target"

# ── Discover fonts ──────────────────────────────────────────────────────────
# Try several known locations for each font family.

def _fc_match(family, style="Regular"):
    """Use fontconfig to find a font file by family and style (exact family match)."""
    try:
        import subprocess
        result = subprocess.run(
            ["fc-list", f"{family}:style={style}", "--format=%{file}\n"],
            capture_output=True, text=True, timeout=5,
        )
        for line in result.stdout.strip().splitlines():
            path = line.strip()
            if path and Path(path).is_file():
                return path
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass
    return None

def _find_font(candidates, fc_queries=None):
    """Return the first path that exists from candidates, or try fontconfig."""
    for p in candidates:
        if Path(p).is_file():
            return p
    if fc_queries:
        for family, style in fc_queries:
            found = _fc_match(family, style)
            if found:
                return found
    return None

# Serif body font: prefer Lora, fall back to Noto Serif, then DejaVu Serif.
_SERIF_CANDIDATES = {
    "regular": [
        "/usr/share/fonts/truetype/google-fonts/Lora-Regular.ttf",
        "/usr/share/fonts/google-noto/NotoSerif-Regular.ttf",
        "/usr/share/fonts/dejavu-serif-fonts/DejaVuSerif.ttf",
    ],
    "bold": [
        "/usr/share/fonts/truetype/google-fonts/Lora-SemiBold.ttf",
        "/usr/share/fonts/google-noto/NotoSerif-Bold.ttf",
        "/usr/share/fonts/dejavu-serif-fonts/DejaVuSerif-Bold.ttf",
    ],
    "italic": [
        "/usr/share/fonts/truetype/google-fonts/Lora-Italic.ttf",
        "/usr/share/fonts/google-noto/NotoSerif-Italic.ttf",
        "/usr/share/fonts/dejavu-serif-fonts/DejaVuSerif-Italic.ttf",
    ],
    "bolditalic": [
        "/usr/share/fonts/truetype/google-fonts/Lora-SemiBoldItalic.ttf",
        "/usr/share/fonts/google-noto/NotoSerif-BoldItalic.ttf",
        "/usr/share/fonts/dejavu-serif-fonts/DejaVuSerif-BoldItalic.ttf",
    ],
}

_SANS_CD_CANDIDATES = {
    "regular": [
        "/usr/share/fonts/truetype/dejavu/DejaVuSansCondensed.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSansCondensed.ttf",
    ],
    "bold": [
        "/usr/share/fonts/truetype/dejavu/DejaVuSansCondensed-Bold.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSansCondensed-Bold.ttf",
    ],
    "oblique": [
        "/usr/share/fonts/truetype/dejavu/DejaVuSansCondensed-Oblique.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSansCondensed-Oblique.ttf",
    ],
    "boldoblique": [
        "/usr/share/fonts/truetype/dejavu/DejaVuSansCondensed-BoldOblique.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSansCondensed-BoldOblique.ttf",
    ],
}

_SANS_CANDIDATES = {
    "regular": [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    ],
    "bold": [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf",
    ],
    "oblique": [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Oblique.ttf",
    ],
    "boldoblique": [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-BoldOblique.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-BoldOblique.ttf",
    ],
}

_SERIF_FC = {
    "regular":    [("Lora", "Regular"), ("Noto Serif", "Regular"), ("DejaVu Serif", "Book")],
    "bold":       [("Lora", "SemiBold"), ("Noto Serif", "Bold"), ("DejaVu Serif", "Bold")],
    "italic":     [("Lora", "Italic"), ("Noto Serif", "Italic"), ("DejaVu Serif", "Italic")],
    "bolditalic": [("Lora", "SemiBold Italic"), ("Noto Serif", "Bold Italic"), ("DejaVu Serif", "Bold Italic")],
}

_SANS_CD_FC = {
    "regular":     [("DejaVu Sans Condensed", "Book"), ("Noto Sans", "Regular")],
    "bold":        [("DejaVu Sans Condensed", "Bold"), ("Noto Sans", "Bold")],
    "oblique":     [("DejaVu Sans Condensed", "Oblique"), ("Noto Sans", "Italic")],
    "boldoblique": [("DejaVu Sans Condensed", "Bold Oblique"), ("Noto Sans", "Bold Italic")],
}

_SANS_FC = {
    "regular":     [("DejaVu Sans", "Book"), ("Noto Sans", "Regular")],
    "bold":        [("DejaVu Sans", "Bold"), ("Noto Sans", "Bold")],
    "oblique":     [("DejaVu Sans", "Oblique"), ("Noto Sans", "Italic")],
    "boldoblique": [("DejaVu Sans", "Bold Oblique"), ("Noto Sans", "Bold Italic")],
}

def _register_fonts():
    serif_r = _find_font(_SERIF_CANDIDATES["regular"], _SERIF_FC["regular"])
    serif_b = _find_font(_SERIF_CANDIDATES["bold"], _SERIF_FC["bold"])
    serif_i = _find_font(_SERIF_CANDIDATES["italic"], _SERIF_FC["italic"])
    serif_bi = _find_font(_SERIF_CANDIDATES["bolditalic"], _SERIF_FC["bolditalic"])
    if not serif_r:
        sys.exit("ERROR: No serif font found (tried Lora, Noto Serif, DejaVu Serif)")

    pdfmetrics.registerFont(RLTTFont("Serif", serif_r))
    pdfmetrics.registerFont(RLTTFont("Serif-Bold", serif_b or serif_r))
    pdfmetrics.registerFont(RLTTFont("Serif-Italic", serif_i or serif_r))
    pdfmetrics.registerFont(RLTTFont("Serif-BoldItalic", serif_bi or serif_r))
    pdfmetrics.registerFontFamily(
        "Serif", normal="Serif", bold="Serif-Bold",
        italic="Serif-Italic", boldItalic="Serif-BoldItalic",
    )

    sans_cd_r = _find_font(_SANS_CD_CANDIDATES["regular"], _SANS_CD_FC["regular"])
    sans_cd_b = _find_font(_SANS_CD_CANDIDATES["bold"], _SANS_CD_FC["bold"])
    sans_cd_o = _find_font(_SANS_CD_CANDIDATES["oblique"], _SANS_CD_FC["oblique"])
    sans_cd_bo = _find_font(_SANS_CD_CANDIDATES["boldoblique"], _SANS_CD_FC["boldoblique"])
    if not sans_cd_r:
        sys.exit("ERROR: No condensed sans font found (tried DejaVu Sans Condensed, Noto Sans)")

    pdfmetrics.registerFont(RLTTFont("DJVSansCd", sans_cd_r))
    pdfmetrics.registerFont(RLTTFont("DJVSansCd-Bold", sans_cd_b or sans_cd_r))
    pdfmetrics.registerFont(RLTTFont("DJVSansCd-Oblique", sans_cd_o or sans_cd_r))
    pdfmetrics.registerFont(RLTTFont("DJVSansCd-BoldOblique", sans_cd_bo or sans_cd_r))
    pdfmetrics.registerFontFamily(
        "DJVSansCd", normal="DJVSansCd", bold="DJVSansCd-Bold",
        italic="DJVSansCd-Oblique", boldItalic="DJVSansCd-BoldOblique",
    )

    sans_r = _find_font(_SANS_CANDIDATES["regular"], _SANS_FC["regular"])
    sans_b = _find_font(_SANS_CANDIDATES["bold"], _SANS_FC["bold"])
    sans_o = _find_font(_SANS_CANDIDATES["oblique"], _SANS_FC["oblique"])
    sans_bo = _find_font(_SANS_CANDIDATES["boldoblique"], _SANS_FC["boldoblique"])
    if not sans_r:
        sys.exit("ERROR: No sans font found (tried DejaVu Sans, Noto Sans)")

    pdfmetrics.registerFont(RLTTFont("DJVSans", sans_r))
    pdfmetrics.registerFont(RLTTFont("DJVSans-Bold", sans_b or sans_r))
    pdfmetrics.registerFont(RLTTFont("DJVSans-Oblique", sans_o or sans_r))
    pdfmetrics.registerFont(RLTTFont("DJVSans-BoldOblique", sans_bo or sans_r))
    pdfmetrics.registerFontFamily(
        "DJVSans", normal="DJVSans", bold="DJVSans-Bold",
        italic="DJVSans-Oblique", boldItalic="DJVSans-BoldOblique",
    )

_register_fonts()

# ── Colours ─────────────────────────────────────────────────────────────────
DARK       = colors.HexColor("#1a1a2e")
ACCENT     = colors.HexColor("#2a5c84")
ACCENT2    = colors.HexColor("#3a7cb8")
LIGHT_RULE = colors.HexColor("#d0dde8")
TABLE_HEAD = colors.HexColor("#2a5c84")
TABLE_ALT  = colors.HexColor("#f0f4f8")
QUOTE_BG   = colors.HexColor("#f4f7fa")
MUTED      = colors.HexColor("#555555")

# ── Page setup ──────────────────────────────────────────────────────────────
PAGE_W, PAGE_H = letter
MARGIN = 0.81 * inch

# ── Styles ──────────────────────────────────────────────────────────────────
def S(name, **kw):
    return ParagraphStyle(name, **kw)

styles = {
    "cover_title": S("cover_title",
        fontName="DJVSansCd-Bold", fontSize=30, leading=38,
        textColor=DARK, alignment=TA_CENTER, spaceAfter=8),
    "cover_sub": S("cover_sub",
        fontName="Serif-Italic", fontSize=13, leading=19,
        textColor=ACCENT, alignment=TA_CENTER, spaceAfter=4),
    "cover_meta": S("cover_meta",
        fontName="DJVSans", fontSize=10, leading=14,
        textColor=MUTED, alignment=TA_CENTER, spaceAfter=2),
    "h1": S("h1",
        fontName="DJVSansCd-Bold", fontSize=18, leading=24,
        textColor=DARK, spaceBefore=24, spaceAfter=8),
    "h2": S("h2",
        fontName="DJVSansCd-Bold", fontSize=13, leading=18,
        textColor=ACCENT, spaceBefore=16, spaceAfter=0),
    "h3": S("h3",
        fontName="DJVSans", fontSize=11, leading=16,
        textColor=ACCENT, spaceBefore=12, spaceAfter=4),
    "body": S("body",
        fontName="Serif", fontSize=10, leading=15,
        textColor=colors.HexColor("#222222"),
        alignment=TA_LEFT, spaceAfter=7,
        allowWidows=0, allowOrphans=0),
    "bold_intro": S("bold_intro",
        fontName="DJVSans", fontSize=10, leading=14,
        textColor=DARK, spaceBefore=4, spaceAfter=2),
    "quote": S("quote",
        fontName="Serif-Italic", fontSize=10.5, leading=16,
        textColor=colors.HexColor("#333333"),
        leftIndent=18, rightIndent=18,
        spaceBefore=10, spaceAfter=10,
        backColor=QUOTE_BG, borderPadding=8,
        allowWidows=0, allowOrphans=0),
    "bullet": S("bullet",
        fontName="Serif", fontSize=10, leading=14,
        textColor=colors.HexColor("#222222"),
        leftIndent=20, firstLineIndent=0,
        spaceBefore=0, spaceAfter=2,
        allowWidows=0, allowOrphans=0),
    "toc_title": S("toc_title",
        fontName="DJVSansCd-Bold", fontSize=12, leading=16,
        textColor=DARK, spaceAfter=6),
    "toc_item": S("toc_item",
        fontName="DJVSans", fontSize=10, leading=16,
        textColor=ACCENT, leftIndent=10, spaceAfter=1),
    "footer": S("footer",
        fontName="Serif", fontSize=8, leading=10,
        textColor=MUTED, alignment=TA_CENTER),
    "footnote_num": S("footnote_num",
        fontName="Serif", fontSize=8.5, leading=13,
        textColor=colors.HexColor("#333333"),
        alignment=TA_CENTER, spaceBefore=0, spaceAfter=0),
    "footnote": S("footnote",
        fontName="Serif", fontSize=8.5, leading=13,
        textColor=colors.HexColor("#333333"),
        spaceBefore=0, spaceAfter=0,
        allowWidows=0, allowOrphans=0),
    "table_header": S("table_header",
        fontName="DJVSansCd-Bold", fontSize=9, leading=12,
        textColor=colors.white, alignment=TA_CENTER),
    "table_cell": S("table_cell",
        fontName="Serif", fontSize=9, leading=12,
        textColor=DARK, alignment=TA_LEFT),
}

toc_styles = {
    "h1": S("toc_h1",
        fontName="DJVSansCd-Bold", fontSize=9, leading=14,
        textColor=DARK, leftIndent=0, spaceBefore=0, spaceAfter=0),
    "h2": S("toc_h2",
        fontName="DJVSans", fontSize=9, leading=13,
        textColor=ACCENT, leftIndent=10, spaceBefore=0, spaceAfter=0),
    "h3": S("toc_h3",
        fontName="Serif-Italic", fontSize=8.5, leading=12,
        textColor=MUTED, leftIndent=20, spaceBefore=0, spaceAfter=0),
}

# ── Header/footer callbacks ─────────────────────────────────────────────────
def make_page_callbacks(header_right):
    """Return (on_first_page, on_page) callbacks with the given header text."""
    def on_page(canvas, doc):
        canvas.saveState()
        canvas.setStrokeColor(ACCENT)
        canvas.setLineWidth(1.5)
        canvas.line(MARGIN, PAGE_H - MARGIN + 18, PAGE_W - MARGIN, PAGE_H - MARGIN + 18)
        canvas.setLineWidth(0.5)
        canvas.setStrokeColor(LIGHT_RULE)
        canvas.line(MARGIN, MARGIN - 16, PAGE_W - MARGIN, MARGIN - 16)
        canvas.setFont("DJVSans", 8)
        canvas.setFillColor(MUTED)
        canvas.drawString(MARGIN, PAGE_H - MARGIN + 24, "PHORETIC SECURITY")
        canvas.drawRightString(PAGE_W - MARGIN, PAGE_H - MARGIN + 24, header_right)
        canvas.drawCentredString(PAGE_W / 2, MARGIN - 28, f"Confidential — Page {doc.page}")
        canvas.restoreState()

    def on_first_page(canvas, doc):
        canvas.saveState()
        canvas.setFillColor(ACCENT)
        canvas.rect(0, PAGE_H - 5, PAGE_W, 5, fill=1, stroke=0)
        canvas.restoreState()

    return on_first_page, on_page

# ── Helpers ─────────────────────────────────────────────────────────────────
def escape_xml(text):
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")

def _short_section(name):
    return re.sub(r'^\d[\d.]*\s+', '', name).strip()

def precompute_fnrefs(lines, refs_dict):
    result = {}
    current_section = ""
    occurrence_counts = {}
    for line in lines:
        stripped = line.strip()
        if re.match(r'^\[\d+\]\s+', stripped):
            continue
        m = re.match(r'^(#{1,3})\s+(.*)', stripped)
        if m and len(m.group(1)) >= 2:
            heading = re.sub(r'\{#[^}]+\}', '', m.group(2)).strip()
            current_section = _short_section(heading)
            continue
        for num in re.findall(r'\[(\d+)\](?!\()', stripped):
            if num not in refs_dict:
                continue
            idx = occurrence_counts.get(num, 0) + 1
            occurrence_counts[num] = idx
            anchor_id = f"fnref-{num}-{idx}"
            result.setdefault(num, []).append((anchor_id, current_section))
    return result

# ── Glyph fallback ──────────────────────────────────────────────────────────
# The body fonts are handsome and incomplete: Lora and Noto Sans have no arrows,
# no set operators, no check mark. Rather than policing which Unicode a document
# may use, register fallback fonts and swap in the first one that has the glyph.

_FALLBACK_FONTS = [
    ("NotoMath", SCRIPT_DIR / "fonts" / "NotoSansMath-Regular.ttf"),
    ("NotoSym2", SCRIPT_DIR / "fonts" / "NotoSansSymbols2-Regular.ttf"),
]

# char -> font name that can render it. Built once, consulted per paragraph.
_GLYPH_FALLBACK = {}


def _covered(font_name):
    try:
        return set(pdfmetrics.getFont(font_name).face.charToGlyph)
    except Exception:
        return set()


def _init_glyph_fallback():
    # INTERSECTION, not union: a glyph is only safe if every primary font has
    # it. Noto Sans lacks >= and != while Lora has them, so a union would call
    # them covered and they would render as boxes in sans contexts.
    primary = _covered("Serif") & _covered("DJVSans")
    if not primary:
        return
    for name, path in _FALLBACK_FONTS:
        if not path.is_file():
            continue
        try:
            pdfmetrics.registerFont(RLTTFont(name, str(path)))
        except Exception:
            continue
        for cp in _covered(name) - primary:
            if cp > 0x7F and cp not in _GLYPH_FALLBACK:
                _GLYPH_FALLBACK[cp] = name


_init_glyph_fallback()


def apply_glyph_fallback(text):
    """Wrap characters the body fonts lack in a font that has them."""
    if not _GLYPH_FALLBACK:
        return text
    out = []
    for ch in text:
        font = _GLYPH_FALLBACK.get(ord(ch))
        out.append(f'<font name="{font}">{ch}</font>' if font else ch)
    return "".join(out)


def inline(text, fnref_counter=None, refs_dict=None):
    # Pull code spans out BEFORE emphasis runs. Otherwise an asterisk inside
    # backticks is read as emphasis and pairs with one in a later span, e.g.
    # `*1.15` then `*1.05` emits interleaved <i>/<font> tags and ReportLab's
    # parser rejects them. Extracting first is also what CommonMark does:
    # code spans bind tighter than emphasis.
    _spans = []

    def _stash(m):
        _spans.append(m.group(1))
        return f'\x00CODE{len(_spans) - 1}\x00'

    text = re.sub(r'`(.+?)`', _stash, text)

    text = re.sub(r'\*\*\*(.+?)\*\*\*', r'<b><i>\1</i></b>', text)
    text = re.sub(r'\*\*(.+?)\*\*', r'<b>\1</b>', text)
    text = re.sub(r'\*(.+?)\*', r'<i>\1</i>', text)

    text = re.sub(
        r'\x00CODE(\d+)\x00',
        lambda m: f'<font name="Courier" size="9">{_spans[int(m.group(1))]}</font>',
        text,
    )

    def replace_fnref(m):
        num = m.group(1)
        if refs_dict and num not in refs_dict:
            return m.group(0)
        anchor = ''
        if fnref_counter is not None:
            fnref_counter[num] = fnref_counter.get(num, 0) + 1
            anchor = f'<a name="fnref-{num}-{fnref_counter[num]}"/>'
        return (
            f'{anchor}<a href="#footnote-{num}" color="{ACCENT.hexval()}">'
            f'<super><font size="7">{num}</font></super></a>'
        )
    text = re.sub(r'\[(\d+)\](?!\()', replace_fnref, text)

    text = re.sub(
        r'\[([^\]]+)\]\(#([^\)]+)\)',
        lambda m: f'<a href="#{m.group(2)}" color="{ACCENT.hexval()}">{m.group(1)}</a>',
        text
    )
    text = re.sub(r'\[([^\]]+)\]\([^\)]+\)', r'\1', text)
    return apply_glyph_fallback(text)

def parse_table(lines, fnref_counter=None, refs_dict=None):
    rows = []
    for line in lines:
        if re.match(r'^\|[-| :]+\|$', line.strip()):
            continue
        cells = [c.strip() for c in line.strip().strip('|').split('|')]
        rows.append(cells)
    if not rows:
        return None

    col_count = max(len(r) for r in rows)
    available = PAGE_W - 2 * MARGIN

    tdata = []
    for i, row in enumerate(rows):
        style = styles["table_header"] if i == 0 else styles["table_cell"]
        cells = []
        for c in row:
            parts = re.split(r'<br\s*/?>', c, flags=re.IGNORECASE)
            parts = [p.strip() for p in parts if p.strip()]
            if len(parts) <= 1 or i == 0:
                cell_text = inline(escape_xml(parts[0]), fnref_counter, refs_dict) if parts else ''
                cells.append(Paragraph(cell_text, style))
            else:
                inner_first = style
                inner_rest = ParagraphStyle(f'cell_spaced_{i}', parent=style, spaceBefore=5)
                paras = []
                for j, p in enumerate(parts):
                    s = inner_first if j == 0 else inner_rest
                    paras.append(Paragraph(inline(escape_xml(p), fnref_counter, refs_dict), s))
                inner = Table([[p] for p in paras], colWidths=None)
                inner.setStyle(TableStyle([
                    ('LEFTPADDING',  (0,0),(-1,-1),0),('RIGHTPADDING',(0,0),(-1,-1),0),
                    ('TOPPADDING',   (0,0),(-1,-1),0),('BOTTOMPADDING',(0,0),(-1,-1),0),
                ]))
                cells.append(inner)
        tdata.append(cells)

    if col_count == 2:
        col_widths = [available * 0.30, available * 0.70]
    elif col_count == 3:
        col_widths = [available * 0.25, available * 0.35, available * 0.40]
    else:
        col_widths = [available / col_count] * col_count

    ts = TableStyle([
        ('BACKGROUND',    (0,0),(-1,0),  TABLE_HEAD),
        ('TEXTCOLOR',     (0,0),(-1,0),  colors.white),
        ('FONTNAME',      (0,0),(-1,0),  'DJVSansCd-Bold'),
        ('ROWBACKGROUNDS',(0,1),(-1,-1), [colors.white, TABLE_ALT]),
        ('GRID',          (0,0),(-1,-1), 0.4, LIGHT_RULE),
        ('VALIGN',        (0,0),(-1,-1), 'MIDDLE'),
        ('TOPPADDING',    (0,0),(-1,-1), 5),
        ('BOTTOMPADDING', (0,0),(-1,-1), 5),
        ('LEFTPADDING',   (0,0),(-1,-1), 8),
        ('RIGHTPADDING',  (0,0),(-1,-1), 8),
    ])

    t = Table(tdata, colWidths=col_widths, repeatRows=1)
    t.setStyle(ts)
    return t

def clean_heading(h):
    h = re.sub(r'[.:;,?]+$', '', h.strip())
    minor = {'a','an','and','as','at','but','by','for','in','nor','of','on','or','so','the','to','up','yet','vs'}
    words = h.split()
    result = []
    for i, w in enumerate(words):
        if w.isupper() and len(w) > 1:
            result.append(w)
        elif w[0:1] == '(' or w.startswith('eBPF'):
            result.append(w)
        elif i == 0 or w.lower() not in minor:
            result.append(w[0].upper() + w[1:] if w else w)
        else:
            result.append(w.lower())
    return ' '.join(result)

# ── Main story builder ──────────────────────────────────────────────────────
def build_story(md_path):
    with open(md_path) as f:
        raw_text = f.read()
        lines = raw_text.split('\n')

    # Extract reference definitions
    refs = {}
    ref_section_start = None
    for idx, line in enumerate(lines):
        m = re.match(r'^\[(\d+)\]\s+(.+)$', line.strip())
        if m:
            if ref_section_start is None:
                ref_section_start = idx
            refs[m.group(1)] = m.group(2).strip()

    if refs:
        print(f"  Found {len(refs)} reference definitions")

    # Remove the references section from lines to process
    if ref_section_start is not None:
        for idx in range(ref_section_start - 1, -1, -1):
            if lines[idx].strip().startswith('## References'):
                ref_section_start = idx
                break
            if lines[idx].strip() == '---' and idx > 0:
                for jdx in range(idx + 1, len(lines)):
                    if lines[jdx].strip():
                        if lines[jdx].strip().startswith('## References'):
                            ref_section_start = idx
                        break
        lines = lines[:ref_section_start]

    fnrefs = precompute_fnrefs(lines, refs)
    fnref_counter = {}

    story = []
    i = 0
    in_table = False
    table_lines = []
    first_h1 = True
    in_cover = True
    past_cover_rule = False

    while i < len(lines):
        raw = lines[i].rstrip('\n')
        stripped = raw.strip()

        if stripped.startswith('<') and stripped.endswith('>'):
            i += 1
            continue

        # Tables
        if stripped.startswith('|'):
            if not in_table:
                in_table = True
                table_lines = []
            table_lines.append(stripped)
            i += 1
            continue
        else:
            if in_table:
                t = parse_table(table_lines, fnref_counter, refs)
                if t:
                    story.append(Spacer(1, 6))
                    story.append(t)
                    story.append(Spacer(1, 8))
                in_table = False
                table_lines = []

        # Horizontal rules
        if re.match(r'^-{3,}$', stripped):
            if in_cover and not past_cover_rule:
                past_cover_rule = True
                i += 1
                continue
            story.append(Spacer(1, 4))
            story.append(HRFlowable(width="100%", thickness=0.5, color=LIGHT_RULE))
            story.append(Spacer(1, 4))
            i += 1
            continue

        # Headings
        m = re.match(r'^(#{1,4})\s+(.*)', stripped)
        if m:
            level = len(m.group(1))
            raw_heading = m.group(2)
            anchor_match = re.search(r'\{#([^}]+)\}', raw_heading)
            anchor_id = anchor_match.group(1) if anchor_match else None
            text = re.sub(r'\{#[^}]+\}', '', raw_heading).strip()
            text = escape_xml(text)
            text = inline(text, fnref_counter, refs)
            if anchor_id:
                text = f'<a name="{anchor_id}"/>{text}'

            if level == 1:
                if first_h1:
                    story.append(Spacer(1, 1.2*inch))
                    story.append(Paragraph(text, styles["cover_title"]))
                    first_h1 = False
                else:
                    in_cover = False
                    story.append(PageBreak())
                    story.append(Paragraph(text, styles["h1"]))
                    story.append(HRFlowable(width="100%", thickness=2, color=ACCENT))
                    story.append(Spacer(1, 4))
            elif level == 2:
                in_cover = False
                if any(text.startswith(s) for s in [
                    "Table of Contents", "Appendix", "Notes"
                ]) or re.match(r'^(1\.|Executive)', text):
                    story.append(PageBreak())
                story.append(Paragraph(text, styles["h2"]))
                story.append(HRFlowable(width="40%", thickness=1, color=LIGHT_RULE, hAlign='LEFT'))
            elif level == 3:
                if in_cover:
                    story.append(Paragraph(text, styles["cover_sub"]))
                    in_cover = False
                else:
                    story.append(Paragraph(text, styles["h3"]))
            elif level == 4:
                story.append(Paragraph(text, styles["bold_intro"]))
            i += 1
            continue

        # Cover subtitle
        if in_cover and stripped.startswith('**') and stripped.endswith('**'):
            text = stripped[2:-2]
            story.append(Paragraph(escape_xml(text), styles["cover_sub"]))
            i += 1
            continue

        # Cover date
        if in_cover and stripped.startswith('*') and stripped.endswith('*') and not stripped.startswith('**'):
            text = stripped[1:-1]
            story.append(Paragraph(escape_xml(text), styles["cover_meta"]))
            i += 1
            continue

        # Blockquote
        if stripped.startswith('>'):
            quote_paras = []
            current_para_lines = []
            while i < len(lines):
                raw2 = lines[i].rstrip('\n')
                s2 = raw2.strip()
                if s2 == '>':
                    if current_para_lines:
                        quote_paras.append(' '.join(current_para_lines))
                        current_para_lines = []
                    i += 1
                elif s2.startswith('>'):
                    text2 = s2.lstrip('> ').strip()
                    # Use the same inline pass as everything else. This
                    # previously ran a lone single-asterisk regex, which turned
                    # **bold** into <i>*bold</i>*, left code spans as literal
                    # backticks, dropped links and footnotes, and -- because it
                    # never reached apply_glyph_fallback -- rendered arrows and
                    # operators as .notdef boxes.
                    text2 = inline(escape_xml(text2), fnref_counter, refs)
                    current_para_lines.append(text2)
                    i += 1
                elif s2 == '':
                    j = i + 1
                    while j < len(lines) and lines[j].strip() == '':
                        j += 1
                    if j < len(lines) and lines[j].strip().startswith('>'):
                        if current_para_lines:
                            quote_paras.append(' '.join(current_para_lines))
                            current_para_lines = []
                        i = j
                    else:
                        break
                else:
                    break
            if current_para_lines:
                quote_paras.append(' '.join(current_para_lines))

            inner_style = styles["quote"].clone("quote_inner")
            inner_style.backColor = None
            inner_style.borderPadding = 0
            flowables = []
            for k, para_text in enumerate(quote_paras):
                p_style = inner_style.clone(f"quote_inner_{k}")
                p_style.spaceAfter = styles["quote"].leading * 0.48 if k < len(quote_paras) - 1 else 0
                flowables.append(Paragraph(para_text, p_style))
            pad = 8
            tbl = Table([[flowables]], colWidths=["100%"])
            tbl.setStyle(TableStyle([
                ("BACKGROUND", (0,0),(-1,-1), QUOTE_BG),
                ("LEFTPADDING", (0,0),(-1,-1), pad + 6),
                ("RIGHTPADDING",(0,0),(-1,-1), pad),
                ("TOPPADDING",  (0,0),(-1,-1), pad),
                ("BOTTOMPADDING",(0,0),(-1,-1), pad),
                ("LINEBEFORE", (0,0),(0,-1), 3, ACCENT),
            ]))
            # Space the callout symmetrically via the flowable's own
            # attributes. This was an explicit Spacer of a full leading before
            # the table and nothing after, so every callout sat low in its gap.
            tbl.spaceBefore = 6
            tbl.spaceAfter = 6
            story.append(tbl)
            continue

        # Bullet lists
        if re.match(r'^[-*]\s+', stripped):
            text = re.sub(r'^[-*]\s+', '', stripped)
            text = inline(escape_xml(text), fnref_counter, refs)
            story.append(Paragraph(f"<bullet>&bull;</bullet> {text}", styles["bullet"]))
            i += 1
            continue

        # Numbered lists
        if re.match(r'^\d+\.\s+', stripped):
            text = re.sub(r'^\d+\.\s+', '', stripped)
            text = inline(escape_xml(text), fnref_counter, refs)
            story.append(Paragraph(f"<bullet>&bull;</bullet> {text}", styles["bullet"]))
            i += 1
            continue

        # Empty lines
        if not stripped:
            story.append(Spacer(1, 5))
            i += 1
            continue

        # Plain paragraph
        text = inline(escape_xml(stripped), fnref_counter, refs)

        if re.match(r'^<b>[^<]+</b>$', text):
            heading = re.sub(r'^<b>|</b>$', '', text)
            story.append(Paragraph(clean_heading(heading), styles["bold_intro"]))
        elif re.match(r'^<b>[^<]+</b>\s+\S', text):
            m_bold = re.match(r'^<b>([^<]+)</b>\s+(.*)', text)
            if m_bold:
                story.append(Paragraph(clean_heading(m_bold.group(1)), styles["bold_intro"]))
                story.append(Paragraph(m_bold.group(2), styles["body"]))
            else:
                story.append(Paragraph(text, styles["body"]))
        else:
            story.append(Paragraph(text, styles["body"]))
        i += 1

    # Endnotes
    if refs:
        story.append(PageBreak())
        story.append(Paragraph("Notes &amp; References", styles["h2"]))
        story.append(HRFlowable(width="40%", thickness=1, color=LIGHT_RULE, hAlign='LEFT'))
        story.append(Spacer(1, 8))

        for num in sorted(refs.keys(), key=int):
            fn_text = escape_xml(refs[num])
            fn_text = re.sub(r'\*(.+?)\*', r'<i>\1</i>', fn_text)
            fn_text = re.sub(r'https?://\S+', '', fn_text).strip()
            back = ''
            occurrences = fnrefs.get(num, [])
            if len(occurrences) == 1:
                anchor_id = occurrences[0][0]
                back = f'  <a href="#{anchor_id}" color="{ACCENT.hexval()}"><font size="7">↑</font></a>'
            elif len(occurrences) > 1:
                parts = []
                for anchor_id, section in occurrences:
                    parts.append(
                        f'<a href="#{anchor_id}" color="{ACCENT.hexval()}">'
                        f'<font size="7">↑ {escape_xml(section)}</font></a>'
                    )
                back = '  ' + '  '.join(parts)

            NUM_COL = 22
            available = PAGE_W - 2 * MARGIN
            num_cell = Paragraph(
                f'<a name="footnote-{num}"/>{num}.',
                styles["footnote_num"],
            )
            txt_cell = Paragraph(fn_text + back, styles["footnote"])
            tbl = Table(
                [[num_cell, txt_cell]],
                colWidths=[NUM_COL, available - NUM_COL],
                spaceBefore=2, spaceAfter=2,
            )
            tbl.setStyle(TableStyle([
                ('VALIGN',       (0,0),(-1,-1), 'TOP'),
                ('LEFTPADDING',  (0,0),(-1,-1), 0),
                ('RIGHTPADDING', (0,0),(-1,-1), 0),
                ('TOPPADDING',   (0,0),(-1,-1), 0),
                ('BOTTOMPADDING',(0,0),(-1,-1), 0),
            ]))
            story.append(tbl)

    return story

# ── Keep-with-next ──────────────────────────────────────────────────────────
HEADING_STYLES = {styles["h1"].name, styles["h2"].name, styles["h3"].name, styles["bold_intro"].name}

def is_heading(flowable):
    return isinstance(flowable, Paragraph) and getattr(flowable.style, 'name', None) in HEADING_STYLES

def keep_with_next(story):
    def is_skippable(fl):
        return isinstance(fl, (Spacer, HRFlowable))
    result = []
    i = 0
    while i < len(story):
        item = story[i]
        if is_heading(item):
            group = [item]
            j = i + 1
            while j < len(story) and (is_heading(story[j]) or is_skippable(story[j])):
                group.append(story[j])
                j += 1
            while j < len(story):
                nxt = story[j]
                group.append(nxt)
                j += 1
                if not is_skippable(nxt):
                    break
            result.append(KeepTogether(group))
            i = j
        else:
            result.append(item)
            i += 1
    return result

# ── TOC ─────────────────────────────────────────────────────────────────────
class PhoDocTemplate(BaseDocTemplate):
    def __init__(self, *args, **kwargs):
        BaseDocTemplate.__init__(self, *args, **kwargs)
        self.toc_entries = []
        self._heading_styles = {'h1', 'h2', 'h3'}

    def afterFlowable(self, flowable):
        if isinstance(flowable, Paragraph):
            sn = getattr(flowable.style, 'name', None)
            if sn in self._heading_styles:
                raw = flowable.text
                raw = re.sub(r'<[^>]+>', '', raw)
                raw = raw.replace('&amp;', '&').replace('&lt;', '<').replace('&gt;', '>')
                text = raw.strip()
                if text:
                    self.toc_entries.append((sn, text, self.page))

class TocPlaceholder(Paragraph):
    def __init__(self, key, level, text, style):
        self.toc_key = key
        self.toc_level = level
        self.toc_text = text
        Paragraph.__init__(self, text, style)

def build_toc_placeholders(story):
    entries = []
    idx = 0
    first_h1 = True
    for item in story:
        items = item._content if isinstance(item, KeepTogether) else [item]
        for fl in items:
            if not isinstance(fl, Paragraph):
                continue
            sn = getattr(fl.style, 'name', None)
            if sn not in ('h1', 'h2', 'h3'):
                continue
            raw = fl.text
            raw = re.sub(r'<[^>]+>', '', raw)
            raw = raw.replace('&amp;', '&').replace('&lt;', '<').replace('&gt;', '>')
            text = raw.strip()
            if not text:
                continue
            if sn == 'h2' and not first_h1:
                entries.append(Spacer(1, 10))
            first_h1 = False
            key = f"toc-{idx}"
            entries.append(TocPlaceholder(key, sn, text, toc_styles[sn]))
            idx += 1
    return entries

def update_toc_with_pages(toc_flowables, toc_entries):
    page_lookup = {}
    for level, text, page in toc_entries:
        page_lookup.setdefault((level, text), page)

    updated = []
    for fl in toc_flowables:
        if isinstance(fl, TocPlaceholder):
            page = page_lookup.get((fl.toc_level, fl.toc_text), '')
            text_escaped = escape_xml(fl.toc_text)
            page_str = str(page) if page else ''
            style = toc_styles[fl.toc_level]
            left_para = Paragraph(text_escaped, style)
            page_style = S(f"toc_page_{fl.toc_level}",
                fontName=style.fontName, fontSize=style.fontSize,
                leading=style.leading, textColor=style.textColor,
                alignment=TA_RIGHT)
            right_para = Paragraph(page_str, page_style)
            avail = (PAGE_W - 2 * MARGIN - 12) / 2
            tbl = Table(
                [[left_para, right_para]],
                colWidths=[avail - 24, 24],
                spaceBefore=style.spaceBefore, spaceAfter=0,
            )
            tbl.setStyle(TableStyle([
                ('VALIGN',       (0,0),(-1,-1),'TOP'),
                ('LEFTPADDING',  (0,0),(0,0), style.leftIndent),
                ('LEFTPADDING',  (1,0),(1,0), 0),
                ('RIGHTPADDING', (0,0),(-1,-1), 0),
                ('TOPPADDING',   (0,0),(-1,-1), 0),
                ('BOTTOMPADDING',(0,0),(-1,-1), 1),
            ]))
            updated.append(tbl)
        else:
            updated.append(fl)
    return updated

def make_toc_table(toc_items):
    mid = (len(toc_items) + 1) // 2
    left_items = toc_items[:mid]
    right_items = toc_items[mid:]

    available = PAGE_W - 2 * MARGIN
    col_w = (available - 18) / 2

    # One row per PAIR of entries rather than one row holding two tall stacks.
    # A single-row table cannot split, so a long contents (the domain model has
    # ~130 entries) overflows the page and ReportLab raises LayoutError. Pairing
    # keeps the identical two-column appearance while letting the table
    # paginate.
    pad = [''] * (len(left_items) - len(right_items))
    rows = [list(pair) for pair in zip(left_items, right_items + pad)]

    tbl = Table(rows, colWidths=[col_w, col_w], repeatRows=0)
    tbl.setStyle(TableStyle([
        ('VALIGN',       (0,0),(-1,-1), 'TOP'),
        ('LEFTPADDING',  (0,0),(0,0), 0),
        ('LEFTPADDING',  (1,0),(1,0), 18),
        ('RIGHTPADDING', (0,0),(-1,-1), 0),
        ('TOPPADDING',   (0,0),(-1,-1), 0),
        ('BOTTOMPADDING',(0,0),(-1,-1), 0),
    ]))
    return tbl

# ── Document metadata ───────────────────────────────────────────────────────
def _extract_title(md_path):
    """Extract the H1 title from the markdown file."""
    with open(md_path) as f:
        for line in f:
            m = re.match(r'^#\s+(.+)', line.strip())
            if m:
                return m.group(1).strip()
    return md_path.stem.replace('_', ' ').title()

def _derive_subtitle(md_path):
    """Derive a short subtitle for the page header from the filename."""
    name = md_path.stem
    # Strip common prefix
    name = re.sub(r'^phoretic_?', '', name)
    mapping = {
        'whitepaper': 'IoT Fleet Security White Paper',
        'overview': 'Overview',
        'roadmap': 'Technical Roadmap — Internal',
        'faq_customers': 'FAQ — Device Manufacturers',
        'faq_investors': 'FAQ — Investors',
        'cofounder_roles': 'Cofounder Roles',
        'presentation': '20-Minute Overview',
    }
    return mapping.get(name, name.replace('_', ' ').title())

# Threshold: documents longer than this many lines get a TOC
TOC_LINE_THRESHOLD = 300

# ── Render one document ─────────────────────────────────────────────────────
def render_doc(md_path, output_path):
    md_path = Path(md_path)
    output_path = Path(output_path)
    output_path.parent.mkdir(parents=True, exist_ok=True)

    title = _extract_title(md_path)
    subtitle = _derive_subtitle(md_path)
    line_count = sum(1 for _ in open(md_path))
    use_toc = line_count > TOC_LINE_THRESHOLD

    print(f"Rendering {md_path.name} → {output_path.name} ({line_count} lines, TOC={'yes' if use_toc else 'no'})")

    on_first_page, on_page = make_page_callbacks(subtitle)
    content_frame = Frame(MARGIN, MARGIN, PAGE_W - 2*MARGIN, PAGE_H - 2*MARGIN, id='content')

    cover_sep = [
        Spacer(1, 0.3*inch),
        HRFlowable(width="60%", thickness=2, color=ACCENT, hAlign='CENTER'),
        Spacer(1, 0.15*inch),
        Paragraph("Confidential — For review only", styles["cover_meta"]),
        Spacer(1, 0.5*inch),
        HRFlowable(width="30%", thickness=1, color=LIGHT_RULE, hAlign='CENTER'),
    ]

    def _make_doc(path):
        doc = PhoDocTemplate(
            str(path),
            pagesize=letter,
            leftMargin=MARGIN, rightMargin=MARGIN,
            topMargin=MARGIN, bottomMargin=MARGIN,
            title=title,
            author="Alex Cruise",
            subject=subtitle,
        )
        doc.addPageTemplates([
            PageTemplate(id='First', frames=[content_frame], onPage=on_first_page),
            PageTemplate(id='Later', frames=[content_frame], onPage=on_page),
        ])
        return doc

    def _insert_cover_sep(story):
        insert_pos = 0
        for idx, el in enumerate(story):
            if isinstance(el, Paragraph):
                sn = getattr(el.style, 'name', '')
                if sn in ('cover_title', 'cover_sub', 'cover_meta'):
                    insert_pos = idx + 1
        return story[:insert_pos] + cover_sep + story[insert_pos:]

    if not use_toc:
        # Simple single-pass build
        story = build_story(md_path)
        story = _insert_cover_sep(story)
        story = keep_with_next(story)
        doc = _make_doc(output_path)
        doc.build(story)
    else:
        # Two-pass build with TOC
        story = build_story(md_path)
        story = _insert_cover_sep(story)
        toc_placeholders = build_toc_placeholders(story)

        toc_header = [
            PageBreak(),
            Paragraph("Table of Contents", styles["h2"]),
            HRFlowable(width="40%", thickness=1, color=LIGHT_RULE, hAlign='LEFT'),
            Spacer(1, 8),
        ]

        def _insert_toc(story, toc_section):
            first_pb = None
            for idx, el in enumerate(story):
                if isinstance(el, PageBreak):
                    first_pb = idx
                    break
            if first_pb is not None:
                return story[:first_pb] + toc_section + story[first_pb:]
            return story

        toc_table = make_toc_table(toc_placeholders)
        full_toc = toc_header + [toc_table]
        story = _insert_toc(story, full_toc)
        story = keep_with_next(story)

        # Pass 1: collect page numbers
        doc_pass1 = _make_doc(output_path)
        doc_pass1.build(copy.deepcopy(story))
        entries = doc_pass1.toc_entries

        # Pass 2: rebuild with page numbers in TOC
        updated_toc = update_toc_with_pages(toc_placeholders, entries)
        updated_table = make_toc_table(updated_toc)
        updated_toc_section = toc_header + [updated_table]

        story2 = build_story(md_path)
        story2 = _insert_cover_sep(story2)
        story2 = _insert_toc(story2, updated_toc_section)
        story2 = keep_with_next(story2)

        doc_pass2 = _make_doc(output_path)
        doc_pass2.build(story2)

    print(f"  ✓ {output_path}")

# ── CLI ─────────────────────────────────────────────────────────────────────
def resolve_targets(args):
    """Resolve CLI arguments to (md_path, pdf_path) pairs."""
    if not args:
        targets = sorted(SRC_DIR.glob("*.md"))
        if not targets:
            sys.exit(f"No .md files found in {SRC_DIR}")
        return [(p, TARGET_DIR / p.with_suffix('.pdf').name) for p in targets]

    pairs = []
    for arg in args:
        p = Path(arg)
        # If it's a direct path to a .md file
        if p.suffix == '.md' and p.is_file():
            pairs.append((p, TARGET_DIR / p.with_suffix('.pdf').name))
            continue
        # Otherwise treat as a short name: "whitepaper" → "strategy_whitepaper.md"
        candidates = [
            SRC_DIR / f"{arg}.md",
            # legacy phoretic naming, kept so short names still work there
            SRC_DIR / f"phoretic_{arg}.md",
            SRC_DIR / f"cofounder_{arg}.md",
        ]
        found = False
        for c in candidates:
            if c.is_file():
                pairs.append((c, TARGET_DIR / c.with_suffix('.pdf').name))
                found = True
                break
        if not found:
            print(f"WARNING: could not find document for '{arg}', skipping")
    return pairs

def main():
    global TARGET_DIR
    args = sys.argv[1:]
    if "--out-dir" in args:
        i = args.index("--out-dir")
        TARGET_DIR = Path(args[i + 1])
        del args[i:i + 2]
    TARGET_DIR.mkdir(parents=True, exist_ok=True)

    targets = resolve_targets(args)
    if not targets:
        sys.exit("No documents to render")

    print(f"Rendering {len(targets)} document(s)...\n")
    for md_path, pdf_path in targets:
        render_doc(md_path, pdf_path)
    print(f"\nDone. Output in {TARGET_DIR}/")

if __name__ == "__main__":
    main()
