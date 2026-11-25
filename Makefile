# Celeritas Makefile
#
#   make            build the release binary
#   make install    build and install the binary, man page and app-menu launcher
#   make install-man      install only the man page
#   make install-desktop  install only the app-menu launcher
#   make uninstall  remove everything that install puts in place
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

.PHONY: all build install install-bin install-man install-desktop uninstall clean

all: build

build: $(BIN)

$(BIN): Cargo.toml Cargo.lock $(wildcard src/*.rs)
	$(CARGO) build --release

MANDIR     := $(PREFIX)/share/man/man1
DESKTOPDIR := $(PREFIX)/share/applications

install: install-bin install-man install-desktop

install-bin: build
	install -d "$(DESTDIR)$(BINDIR)"
	install -m 755 "$(BIN)" "$(DESTDIR)$(BINDIR)/$(NAME)"
	@echo "Installed $(NAME) to $(DESTDIR)$(BINDIR)/$(NAME)"
	@case ":$$PATH:" in *":$(BINDIR):"*) ;; \
	  *) echo "Note: $(BINDIR) is not on your PATH." ;; esac

install-man:
	install -d "$(DESTDIR)$(MANDIR)"
	install -m 644 man/$(NAME).1 "$(DESTDIR)$(MANDIR)/$(NAME).1"
	@echo "Installed man page to $(DESTDIR)$(MANDIR)/$(NAME).1"

install-desktop:
	install -d "$(DESTDIR)$(DESKTOPDIR)"
	sed -e 's|@BINDIR@|$(BINDIR)|g' dist/$(NAME).desktop.in > "$(DESTDIR)$(DESKTOPDIR)/$(NAME).desktop"
	@echo "Installed launcher to $(DESTDIR)$(DESKTOPDIR)/$(NAME).desktop"

uninstall:
	rm -f "$(DESTDIR)$(BINDIR)/$(NAME)"
	rm -f "$(DESTDIR)$(MANDIR)/$(NAME).1"
	rm -f "$(DESTDIR)$(DESKTOPDIR)/$(NAME).desktop"
	@echo "Removed $(NAME) from $(DESTDIR)$(PREFIX)"

clean:
	$(CARGO) clean
