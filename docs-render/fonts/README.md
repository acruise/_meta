# Vendored fallback fonts

The body fonts (Lora, Noto Sans) are handsome and do not cover arrows, set
operators, or a few symbols -- `->`, `>=`, `in`, `union`, `check`, `warning` all
render as .notdef boxes. These two cover the gap:

- `NotoSansMath-Regular.ttf`     arrows, comparisons, set operators
- `NotoSansSymbols2-Regular.ttf` check mark, warning sign

Both are SIL Open Font License 1.1, from github.com/google/fonts, and are
vendored rather than downloaded at build time so rendering is deterministic and
works offline.

`render.py` registers them as fallbacks and wraps any character missing from the
primary fonts in the first fallback that has it, so documents can use whatever
Unicode reads best without anyone checking coverage by hand.
