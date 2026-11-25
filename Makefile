# celeritas Makefile
#
#   make            build the release binary
#   make install    build and install to $(PREFIX)/bin (default: ~/.local/bin, no sudo needed)
#   make install-desktop  also add an app-menu launcher
#   make uninstall  remove the installed binary (and launcher)
#   make clean      remove build artifacts
#
# Override the install location, e.g.:
#   make install PREFIX=/usr/local      (may need sudo)

NAME    := celeritas
PREFIX  ?= $(HOME)/.local
BINDIR  ?= $(PREFIX)/bin
DESTDIR ?=
CARGO   ?= cargo

BIN     := target/release/$(NAME)

.PHONY: all build install install-desktop uninstall clean

all: build

build: $(BIN)

$(BIN): Cargo.toml Cargo.lock $(wildcard src/*.rs)
	$(CARGO) build --release

install: build
	install -d "$(DESTDIR)$(BINDIR)"
	install -m 755 "$(BIN)" "$(DESTDIR)$(BINDIR)/$(NAME)"
	@echo "Installed $(NAME) to $(DESTDIR)$(BINDIR)/$(NAME)"
	@case ":$$PATH:" in *":$(BINDIR):"*) ;; \
	  *) echo "Note: $(BINDIR) is not on your PATH." ;; esac

DESKTOPDIR := $(PREFIX)/share/applications

install-desktop: install
	install -d "$(DESTDIR)$(DESKTOPDIR)"
	sed -e 's|@BINDIR@|$(BINDIR)|g' dist/$(NAME).desktop.in > "$(DESTDIR)$(DESKTOPDIR)/$(NAME).desktop"
	@echo "Installed launcher to $(DESTDIR)$(DESKTOPDIR)/$(NAME).desktop"

uninstall:
	rm -f "$(DESTDIR)$(BINDIR)/$(NAME)"
	rm -f "$(DESTDIR)$(DESKTOPDIR)/$(NAME).desktop"
	@echo "Removed $(DESTDIR)$(BINDIR)/$(NAME)"

clean:
	$(CARGO) clean
