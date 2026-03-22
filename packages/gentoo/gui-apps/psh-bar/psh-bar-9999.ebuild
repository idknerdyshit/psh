# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

RUST_MIN_VER="1.85.0"

inherit cargo git-r3 systemd

DESCRIPTION="System bar for the psh Wayland desktop environment"
HOMEPAGE="https://github.com/idknerdyshit/psh"
EGIT_REPO_URI="https://github.com/idknerdyshit/psh.git"

LICENSE="GPL-3+"
SLOT="0"

DEPEND="
	gui-libs/gtk:4
	gui-libs/gtk4-layer-shell
	sys-apps/dbus
"
RDEPEND="${DEPEND}"

QA_FLAGS_IGNORED="usr/bin/psh-bar"

src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

src_compile() {
	cargo_src_compile --bin psh-bar
}

src_install() {
	dobin "$(cargo_target_dir)/psh-bar"
	systemd_douserunit "${S}/systemd/psh-bar.service"
}

pkg_postinst() {
	elog "psh-bar is the IPC hub — all other psh components connect to it."
	elog "It must be running before other components can communicate."
	elog ""
	elog "Configure modules in ~/.config/psh/psh.toml under [bar]."
	elog "See: https://github.com/idknerdyshit/psh#bar"
}
