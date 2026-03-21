# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

CRATES=""

inherit cargo systemd

DESCRIPTION="Polkit authentication agent for the psh Wayland desktop environment"
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

src_compile() {
	cargo_src_compile --bin psh-polkit
}

src_install() {
	dobin "$(cargo_target_dir)/psh-polkit"
	systemd_douserunit "${S}/systemd/psh-polkit.service"
}
