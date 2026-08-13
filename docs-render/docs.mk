# Shared markdown -> PDF build, included by consuming projects.
#
# Usage, from a project's docs/Makefile:
#
#     META_RENDER := ../../_meta/docs-render
#     include $(META_RENDER)/docs.mk
#
# Layout it expects, relative to the including Makefile:
#
#     src/*.md  ->  target/*.pdf
#
# Pure ReportLab: no browser, no LaTeX, no pandoc. Fonts are vendored beside
# render.py so output is deterministic and works offline.

SRC    ?= src
TARGET ?= target
PYTHON ?= python3

MD_FILES  := $(wildcard $(SRC)/*.md)
PDF_FILES := $(patsubst $(SRC)/%.md,$(TARGET)/%.pdf,$(MD_FILES))

.PHONY: all open clean
.DEFAULT_GOAL := all

all: $(PDF_FILES)

$(TARGET)/%.pdf: $(SRC)/%.md $(META_RENDER)/render.py
	@mkdir -p $(TARGET)
	$(PYTHON) $(META_RENDER)/render.py $< --out-dir $(TARGET)

open: all
	@open $(PDF_FILES)

clean:
	rm -rf $(TARGET)
