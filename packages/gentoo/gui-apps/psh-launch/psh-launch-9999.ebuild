# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

RUST_MIN_VER="1.85.0"

inherit cargo git-r3 systemd

DESCRIPTION="Application launcher for the psh Wayland desktop environment"
HOMEPAGE="https://github.com/idknerdyshit/psh"
EGIT_REPO_URI="https://github.com/idknerdyshit/psh.git"

LICENSE="GPL-3+"
SLOT="0"

DEPEND="
	gui-libs/gtk:4
	gui-libs/gtk4-layer-shell
"
RDEPEND="${DEPEND}"

QA_FLAGS_IGNORED="usr/bin/psh-launch"

src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

src_compile() {
	cargo_src_compile --bin psh-launch
}

src_install() {
	dobin "$(cargo_target_dir)/psh-launch"
	systemd_douserunit "${S}/systemd/psh-launch.service"
}

pkg_postinst() {
	elog "psh-launch is a long-lived daemon toggled via IPC from psh-bar."
	elog "Bind a key in your compositor to: psh launcher"
}
