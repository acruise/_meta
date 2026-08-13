# docs-render

Markdown to PDF, shared across the cluonflux projects. Pure ReportLab: no
browser, no LaTeX, no pandoc, and no network at build time.

    docs/Makefile:
        META_RENDER := ../../_meta/docs-render
        include $(META_RENDER)/docs.mk

    src/*.md  ->  target/*.pdf

Lives here because it is finicky in ways that are expensive to rediscover. Three
bugs surfaced within an hour of putting a large document through it, each of
which would bite any consumer eventually:

- **Code spans were not protected from emphasis.** Emphasis rules ran before the
  backtick rule, so an asterisk inside backticks paired with one in a later span
  and emitted interleaved tags that ReportLab rejects. Any document with `*` in a
  code span hits this. Code spans are now extracted first, which is also what
  CommonMark specifies.
- **The two-column table of contents could not paginate.** It was a single table
  row holding two tall stacks, and a row cannot split, so a contents past roughly
  130 entries overflowed the page and raised LayoutError. Now one row per pair of
  entries: same appearance, splits naturally.
- **No glyph fallback.** The body fonts (Lora, Noto Sans) are handsome and have
  no arrows, set operators, check mark or warning sign, so those rendered as
  .notdef boxes. `fonts/` now vendors Noto Sans Math and Noto Sans Symbols 2, and
  any character missing from the primary fonts is wrapped in the first fallback
  that has it -- about 5,000 codepoints. Note the primary coverage set is the
  INTERSECTION of the serif and sans faces, not the union: Lora has `>=` and Noto
  Sans does not, so a union would call it covered and it would box in tables.

## reflow.py

Converts hard-wrapped Markdown to soft wraps, one physical line per block. The
renderer assumes soft wraps, and hard wraps break it in a specific way worth
knowing: `inline()` runs per line inside blockquotes, so emphasis or a code span
straddling a wrap never meets its closing delimiter and comes out mangled.
Converting a 4,700-line hard-wrapped document also cut its PDF from 144 pages to
113, the difference being spurious paragraph breaks.

    python3 ../_meta/docs-render/reflow.py in.md out.md

It refuses to write if the content drifts: it compares whitespace-normalised
text with blockquote markers stripped, and exits naming the divergence. Table
rows, fenced code, headings and rules pass through untouched; list continuations
join into their bullet; blockquotes join within a paragraph and honour a bare
`>` as a break.

Worth running on a branch or its own commit, and worth diffing the structural
counts afterwards -- headings, table rows, fences, bullets, blank lines should
all be identical, and only blockquote line counts should fall.

## Fonts

Fonts are SIL Open Font License 1.1, from github.com/google/fonts, vendored
rather than fetched at build time.

Not a codegen input, so it does not participate in EXPR_GEN_HASH.
