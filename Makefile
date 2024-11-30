PREFIX ?=			/usr/local
SYSCONFDIR ?=		/etc
SYSTEMDUNITDIR ?=	$(PREFIX)/lib/systemd/system
SBINDIR ?= 			$(PREFIX)/sbin

debug:
	cargo build

release:
	cargo build --release

user:
	useradd -r -s /bin/false prelockd-rs || true

unit:
	sed 's|@SBINDIR@|$(SBINDIR)|' prelockd-rs.service.in > target/prelockd-rs.service
	install -d $(SYSTEMDUNITDIR)
	install -m 644 target/prelockd-rs.service $(SYSTEMDUNITDIR)/

install: target/release/prelockd-rs user unit
	install -d $(SBINDIR)
	install -m 755 target/release/prelockd-rs $(SBINDIR)/
	install -d $(SYSCONFDIR)
	install -m 644 prelockd-rs.toml $(SYSCONFDIR)/

install/debug: target/debug/prelockd-rs user unit
	install -d $(SBINDIR)
	install -m 755 target/debug/prelockd-rs $(SBINDIR)/
	install -d $(SYSCONFDIR)
	install -m 644 prelockd-rs.toml $(SYSCONFDIR)/

