# Install logic, shared by the AUR package, the Debian package, and a plain
# `make install`. Keeping it in one place means the three can't drift apart.
#
# Honours the usual DESTDIR/PREFIX conventions:
#   make install                       -> /usr/local
#   make install PREFIX=/usr           -> /usr        (what distro packages use)
#   make install DESTDIR=/tmp/pkg      -> staged into a build root
#   make install-user                  -> ~/.local, no root needed

PREFIX ?= /usr/local
DESTDIR ?=

BINDIR = $(DESTDIR)$(PREFIX)/bin
DATADIR = $(DESTDIR)$(PREFIX)/share
APPDIR = $(DATADIR)/applications
ICONDIR = $(DATADIR)/icons/hicolor
LICENSEDIR = $(DATADIR)/licenses/nexo-client

CARGO ?= cargo
ICON_SIZES = 16 24 32 48 64 128 256 512

.PHONY: all build install install-user uninstall uninstall-user clean check

all: build

build:
	$(CARGO) build --release --locked

check:
	$(CARGO) test --locked
	$(CARGO) clippy --all-targets -- -D warnings

# System-wide install. Deliberately does not run update-desktop-database or
# gtk-update-icon-cache: package managers own those caches and run the hooks
# themselves, and touching them from a staged DESTDIR build is wrong.
install:
	install -Dm755 target/release/nexo $(BINDIR)/nexo
	install -Dm644 assets/nexo.desktop $(APPDIR)/nexo.desktop
	install -Dm644 assets/nexo.svg $(ICONDIR)/scalable/apps/nexo.svg
	for size in $(ICON_SIZES); do \
		install -Dm644 assets/icons/$$size.png \
			$(ICONDIR)/$${size}x$${size}/apps/nexo.png; \
	done
	install -Dm644 LICENSE $(LICENSEDIR)/LICENSE

uninstall:
	rm -f $(BINDIR)/nexo
	rm -f $(APPDIR)/nexo.desktop
	rm -f $(ICONDIR)/scalable/apps/nexo.svg
	for size in $(ICON_SIZES); do \
		rm -f $(ICONDIR)/$${size}x$${size}/apps/nexo.png; \
	done
	rm -rf $(LICENSEDIR)

# Per-user install for people running from a source checkout. This one *does*
# refresh the caches, since no package manager is involved to do it.
install-user: build
	$(MAKE) install PREFIX=$(HOME)/.local
	# Absolute Exec/TryExec for this variant only. The desktop session's PATH
	# doesn't always include ~/.local/bin, and a bare `Exec=nexo` that can't
	# be resolved fails silently from the menu — no window, no error.
	sed -i 's|^Exec=nexo$$|Exec=$(HOME)/.local/bin/nexo|; \
	        s|^TryExec=nexo$$|TryExec=$(HOME)/.local/bin/nexo|' \
	    $(HOME)/.local/share/applications/nexo.desktop
	-update-desktop-database $(HOME)/.local/share/applications 2>/dev/null
	-gtk-update-icon-cache -f -t $(HOME)/.local/share/icons/hicolor 2>/dev/null
	-kbuildsycoca6 2>/dev/null
	@echo
	@echo "Installed to $(HOME)/.local — 'Nexo' should now appear under Games."

uninstall-user:
	$(MAKE) uninstall PREFIX=$(HOME)/.local
	-update-desktop-database $(HOME)/.local/share/applications 2>/dev/null
	-kbuildsycoca6 2>/dev/null

clean:
	$(CARGO) clean
