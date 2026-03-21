# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

CRATES=""

inherit cargo systemd

DESCRIPTION="Notification daemon for the psh Wayland desktop environment"
HOMEPAGE="https://github.com/idknerdyshit/psh"
SRC_URI="https://github.com/idknerdyshit/psh/archive/v${PV}.tar.gz -> psh-${PV}.tar.gz"

S="${WORKDIR}/psh-${PV}"

LICENSE="GPL-3+"
SLOT="0"
KEYWORDS="~amd64"

DEPEND="
	gui-libs/gtk:4
	gui-libs/gtk4-layer-shell
	sys-apps/dbus
"
# psh-notify claims org.freedesktop.Notifications on D-Bus.
# Only one notification daemon can own this name at a time.
RDEPEND="
	${DEPEND}
	!!x11-misc/dunst
	!!gui-apps/mako
"
PDEPEND="virtual/notification-daemon"
BDEPEND="virtual/rust"

QA_FLAGS_IGNORED="usr/bin/psh-notify"

src_compile() {
	cargo_src_compile --bin psh-notify
}

src_install() {
	dobin "$(cargo_target_dir)/psh-notify"
	systemd_douserunit "${S}/systemd/psh-notify.service"
}
