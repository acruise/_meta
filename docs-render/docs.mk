# Shared markdown -> PDF build, included by consuming projects.
#
# Usage, from a project's docs/Makefile:
#
#     META_RENDER := ../../_meta/docs-render
#     DECKS       := my-pitch-deck          # optional, see below
#     include $(META_RENDER)/docs.mk
#
# Layout it expects, relative to the including Makefile:
#
#     src/*.md  ->  target/*.pdf
#
# Pure ReportLab: no browser, no LaTeX, no pandoc. Fonts are vendored beside
# render.py so output is deterministic and works offline.
#
# DECKS names documents that are slide decks rather than prose, given without
# the src/ prefix or the .md suffix. Those build through marpify.py and marp
# instead, to target/%.html (standard marp output, presenter notes included)
# and target/%.pdf (slides only). Marp drives headless Chrome, so a deck build
# is neither offline nor fast; that is the price of slides, and it is why only
# the documents named here pay it.

SRC    ?= src
TARGET ?= target
PYTHON ?= python3
MARP   ?= marp

DECKS ?=

MD_FILES   := $(wildcard $(SRC)/*.md)
DECK_MD    := $(patsubst %,$(SRC)/%.md,$(DECKS))
PROSE_MD   := $(filter-out $(DECK_MD),$(MD_FILES))
PDF_FILES  := $(patsubst $(SRC)/%.md,$(TARGET)/%.pdf,$(PROSE_MD))
DECK_MARP  := $(patsubst %,$(TARGET)/%.marp.md,$(DECKS))
DECK_HTML  := $(patsubst %,$(TARGET)/%.html,$(DECKS))
DECK_PDF   := $(patsubst %,$(TARGET)/%.pdf,$(DECKS))

.PHONY: all open clean
.DEFAULT_GOAL := all

all: $(PDF_FILES) $(DECK_HTML) $(DECK_PDF)

$(TARGET)/%.pdf: $(SRC)/%.md $(META_RENDER)/render.py
	@mkdir -p $(TARGET)
	$(PYTHON) $(META_RENDER)/render.py $< --out-dir $(TARGET)

ifneq ($(strip $(DECKS)),)

# Static pattern rules, not plain pattern rules: these are explicit rules for
# named targets, so target/deck.pdf resolves here rather than to the prose rule
# above, which would otherwise match it just as well.

$(DECK_MARP): $(TARGET)/%.marp.md: $(SRC)/%.md $(META_RENDER)/marpify.py
	@mkdir -p $(TARGET)
	$(PYTHON) $(META_RENDER)/marpify.py $< -o $@ $(MARPIFY_FLAGS)

$(DECK_HTML): $(TARGET)/%.html: $(TARGET)/%.marp.md
	@command -v $(MARP) >/dev/null || { echo "$(MARP) not found: npm i -g @marp-team/marp-cli"; exit 1; }
	$(MARP) $< -o $@

# No --pdf-notes, so the notes marpify preserved stay in the HTML's presenter
# view and out of the printed slides.
$(DECK_PDF): $(TARGET)/%.pdf: $(TARGET)/%.marp.md
	@command -v $(MARP) >/dev/null || { echo "$(MARP) not found: npm i -g @marp-team/marp-cli"; exit 1; }
	$(MARP) $< --pdf -o $@

# Keep the intermediate: it is what marp actually saw, so a layout surprise is
# diagnosable without re-running the preprocessor by hand.
.SECONDARY: $(DECK_MARP)

endif

open: all
	@open $(PDF_FILES) $(DECK_PDF)

clean:
	rm -rf $(TARGET)
