#!/usr/bin/env python3
"""Convert hard-wrapped Markdown to soft wraps (one physical line per block).

Joins continuation lines within a block; never joins across block boundaries.
"""
import re, sys

FENCE = re.compile(r'^\s*(```|~~~)')
HEADING = re.compile(r'^\s*#{1,6}\s')
RULE = re.compile(r'^\s*([-*_])\s*(\1\s*){2,}$')
TABLE = re.compile(r'^\s*\|')
BULLET = re.compile(r'^(\s*)([-*+]|\d+[.)])\s+')
QUOTE = re.compile(r'^\s*>')

def is_boundary(l):
    return (not l.strip() or HEADING.match(l) or RULE.match(l)
            or TABLE.match(l) or FENCE.match(l))

def reflow(text):
    lines = text.split('\n')
    out, i, in_fence = [], 0, False
    while i < len(lines):
        l = lines[i]

        if FENCE.match(l):
            in_fence = not in_fence
            out.append(l); i += 1; continue
        if in_fence:
            out.append(l); i += 1; continue

        # boundaries and table rows pass through untouched
        if is_boundary(l):
            out.append(l); i += 1; continue

        # blockquote: join continuations, keep one marker, honour ">" as break
        if QUOTE.match(l):
            indent = re.match(r'^(\s*)', l).group(1)
            parts = [re.sub(r'^\s*>\s?', '', l)]
            i += 1
            while i < len(lines) and QUOTE.match(lines[i]):
                inner = re.sub(r'^\s*>\s?', '', lines[i])
                if not inner.strip():           # ">" alone: paragraph break
                    break
                if BULLET.match(inner) or HEADING.match(inner) or TABLE.match(inner):
                    break
                parts.append(inner.strip()); i += 1
            out.append(indent + '> ' + ' '.join(p.strip() for p in parts if p.strip()))
            continue

        # list item: absorb indented continuations, but not sibling items
        m = BULLET.match(l)
        if m:
            parts = [l.rstrip()]
            base = len(m.group(1))
            i += 1
            while i < len(lines):
                n = lines[i]
                if (not n.strip() or is_boundary(n) or QUOTE.match(n)
                        or BULLET.match(n)):
                    break
                if len(n) - len(n.lstrip()) <= base:   # dedented: new block
                    break
                parts.append(n.strip()); i += 1
            out.append(' '.join(parts))
            continue

        # plain paragraph
        parts = [l.rstrip()]
        i += 1
        while i < len(lines):
            n = lines[i]
            if (not n.strip() or is_boundary(n) or QUOTE.match(n)
                    or BULLET.match(n)):
                break
            parts.append(n.strip()); i += 1
        out.append(' '.join(p.strip() if k else p for k, p in enumerate(parts)))
        continue

    return '\n'.join(out)

if __name__ == '__main__':
    src = sys.argv[1]
    text = open(src).read()
    new = reflow(text)

    # content must be byte-identical once whitespace is normalised
    def toks(s):
        s = re.sub(r'^\s*>\s?', '', s, flags=re.M)
        return re.sub(r'\s+', ' ', s).strip()
    if toks(text) != toks(new):
        a, b = toks(text), toks(new)
        for k in range(min(len(a), len(b))):
            if a[k] != b[k]:
                sys.exit(f"CONTENT DRIFT at {k}:\n  old: ...{a[max(0,k-90):k+90]}...\n  new: ...{b[max(0,k-90):k+90]}...")
        sys.exit(f"CONTENT DRIFT: length {len(a)} -> {len(b)}")

    open(sys.argv[2], 'w').write(new)
    print(f"lines {len(text.splitlines())} -> {len(new.splitlines())}")
