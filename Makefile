# ─────────────────────────────────────────────────────────────────────────────
# beelauncher — build & install
#
#   make              → compile release binary (no root)
#   sudo make install → copy pre-built binary to $(BINDIR)
#   sudo make uninstall
#   make clean
# ─────────────────────────────────────────────────────────────────────────────

BINARY  := beelauncher
CARGO   := cargo

# Destination — override with: make install PREFIX=/usr/local
PREFIX  := /usr
DESTDIR :=
BINDIR  := $(DESTDIR)$(PREFIX)/bin

BIN := target/release/$(BINARY)

# ── Targets ───────────────────────────────────────────────────────────────────

.PHONY: all build install uninstall clean

all: build

build:
	$(CARGO) build --release

# install never calls cargo — it only copies what 'make build' already produced.
# This avoids the sudo PATH problem (cargo lives in ~/.cargo/bin, not root's PATH).
install:
	@test -f $(BIN) || { echo "error: run 'make build' before 'sudo make install'"; exit 1; }
	install -Dm755 $(BIN) $(BINDIR)/$(BINARY)

uninstall:
	rm -f $(BINDIR)/$(BINARY)

clean:
	$(CARGO) clean
