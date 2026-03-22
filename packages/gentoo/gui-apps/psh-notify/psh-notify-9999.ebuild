# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

inherit cargo git-r3 systemd

DESCRIPTION="Notification daemon for the psh Wayland desktop environment"
HOMEPAGE="https://github.com/idknerdyshit/psh"
EGIT_REPO_URI="https://github.com/idknerdyshit/psh.git"

LICENSE="GPL-3+"
SLOT="0"

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

QA_FLAGS_IGNORED="usr/bin/psh-notify"

src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

src_compile() {
	cargo_src_compile --bin psh-notify
}

src_install() {
	dobin "$(cargo_target_dir)/psh-notify"
	systemd_douserunit "${S}/systemd/psh-notify.service"
}

pkg_postinst() {
	elog "psh-notify claims org.freedesktop.Notifications on the session bus."
	elog "Only one notification daemon can run at a time."
	elog ""
	elog "Test with: notify-send \"Hello\" \"from psh-notify\""
}
