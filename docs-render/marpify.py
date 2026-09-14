#!/usr/bin/env python3
"""Prepare a speaker-notes markdown deck for marp.

    python3 marpify.py src/deck.md -o target/deck.marp.md

The source convention is a document that reads as prose and presents as slides:
`---` separates slides, each slide opens with a `## Title`, and the words to say
live under a `**Notes:**` line at the foot of the slide. Marp has no notion of
that convention -- it renders the notes onto the slide, where they overflow --
so this rewrites each notes block as an HTML comment, which is what marp reads
as a presenter note.

The output keeps the notes rather than dropping them, so one intermediate feeds
both outputs: HTML carries them into the presenter view, and a PDF built without
--pdf-notes leaves them out.
"""
import argparse
import re
import sys
from pathlib import Path

SLIDE_SEP = re.compile(r'^-{3,}\s*$')
# Anchored to a whole line on purpose. A deck that documents its own format
# mentions `**Notes:**` mid-sentence inside backticks, and that is prose.
NOTES_MARK = re.compile(r'^\*\*Notes:\*\*\s*$')
FENCE = re.compile(r'^(\s*)(`{3,}|~{3,})')
FRONT_MATTER = re.compile(r'^---\s*$')

DEFAULT_FRONT_MATTER = """---
marp: true
paginate: true
theme: default
style: |
  section { font-size: 25px; padding: 44px 56px; }
  section.lead { text-align: center; }
  h2 { font-size: 36px; margin-bottom: 6px; }
  h3 { font-size: 24px; font-weight: 400; color: #2a5c84; margin-top: 0; }
  table { font-size: 19px; }
  th { background: #2a5c84; color: #fff; }
  pre, code { font-size: 18px; }
  ul, ol { line-height: 1.35; }
---
"""


def split_slides(lines):
    """Split on `---`, ignoring separators inside fenced code."""
    slides, current, fence = [], [], None
    for line in lines:
        if fence is not None:
            current.append(line)
            if line.strip().startswith(fence) and not line.strip().lstrip(fence[0]).strip():
                fence = None
            continue
        m = FENCE.match(line)
        if m:
            fence = m.group(2)
            current.append(line)
            continue
        if SLIDE_SEP.match(line):
            slides.append(current)
            current = []
            continue
        current.append(line)
    slides.append(current)
    return slides


def comment_notes(slide):
    """Turn a trailing `**Notes:**` block into an HTML comment."""
    idx = next((i for i, l in enumerate(slide) if NOTES_MARK.match(l)), None)
    if idx is None:
        return slide
    body = [l for l in slide[idx + 1:]]
    while body and not body[0].strip():
        body.pop(0)
    while body and not body[-1].strip():
        body.pop()
    if not body:
        return slide[:idx]
    # A note cannot contain the comment terminator without ending itself early.
    body = [l.replace('-->', '--&gt;') for l in body]
    head = slide[:idx]
    while head and not head[-1].strip():
        head.pop()
    return head + ['', '<!--', *body, '-->']


def make_title_slide(slide):
    """Reduce a non-slide preamble to its H1, as a lead-classed title slide."""
    title = next((l for l in slide if l.startswith('# ')), None)
    if title is None:
        return slide
    return ['<!-- _class: lead -->', '', title]


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('source', type=Path)
    ap.add_argument('-o', '--out', type=Path, required=True)
    ap.add_argument('--drop-preamble', action='store_true',
                    help="The first block is document front matter rather than a "
                         "slide: keep only its H1, as a title slide.")
    args = ap.parse_args()

    text = args.source.read_text()
    lines = text.split('\n')

    if FRONT_MATTER.match(lines[0] if lines else ''):
        sys.exit(f"{args.source}: already has YAML front matter; "
                 "marpify would not know whether to merge or replace it")

    slides = split_slides(lines)
    if args.drop_preamble and slides and not any(l.startswith('## ') for l in slides[0]):
        slides[0] = make_title_slide(slides[0])

    slides = [comment_notes(s) for s in slides]

    out = [DEFAULT_FRONT_MATTER.rstrip('\n')]
    for i, slide in enumerate(slides):
        body = '\n'.join(slide).strip('\n')
        out.append(('\n---\n\n' if i else '\n') + body)

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text('\n'.join(out).rstrip('\n') + '\n')

    noted = sum(1 for s in slides if any(l == '<!--' for l in s))
    print(f"  {args.source.name} → {args.out.name}: "
          f"{len(slides)} slides, {noted} with presenter notes")


if __name__ == '__main__':
    main()
