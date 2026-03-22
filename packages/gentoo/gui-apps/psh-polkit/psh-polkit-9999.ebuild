# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

inherit cargo git-r3 systemd

DESCRIPTION="Polkit authentication agent for the psh Wayland desktop environment"
HOMEPAGE="https://github.com/idknerdyshit/psh"
EGIT_REPO_URI="https://github.com/idknerdyshit/psh.git"

LICENSE="GPL-3+"
SLOT="0"

DEPEND="
	gui-libs/gtk:4
	gui-libs/gtk4-layer-shell
	sys-apps/dbus
	sys-auth/polkit
"
# psh-polkit registers as the session polkit authentication agent.
# Only one agent can be registered per session.
RDEPEND="
	${DEPEND}
	!!gnome-extra/polkit-gnome
	!!kde-plasma/polkit-kde-agent
	!!lxqt-base/lxqt-policykit
"
BDEPEND="virtual/rust"

QA_FLAGS_IGNORED="usr/bin/psh-polkit"

src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

src_compile() {
	cargo_src_compile --bin psh-polkit
}

src_install() {
	dobin "$(cargo_target_dir)/psh-polkit"
	systemd_douserunit "${S}/systemd/psh-polkit.service"
}

pkg_postinst() {
	elog "psh-polkit registers as the session polkit authentication agent."
	elog "Only one polkit agent can be active per session."
	elog ""
	elog "Test with: pkexec ls"
}
