# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

inherit cargo git-r3 systemd

DESCRIPTION="Clipboard manager for the psh Wayland desktop environment"
HOMEPAGE="https://github.com/idknerdyshit/psh"
EGIT_REPO_URI="https://github.com/idknerdyshit/psh.git"

LICENSE="GPL-3+"
SLOT="0"

DEPEND="
	gui-libs/gtk:4
	gui-libs/gtk4-layer-shell
"
RDEPEND="${DEPEND}"
BDEPEND="virtual/rust"

QA_FLAGS_IGNORED="usr/bin/psh-clip"

src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

src_compile() {
	cargo_src_compile --bin psh-clip
}

src_install() {
	dobin "$(cargo_target_dir)/psh-clip"
	systemd_douserunit "${S}/systemd/psh-clip.service"
}

pkg_postinst() {
	elog "psh-clip monitors the clipboard via zwlr-data-control-v1."
	elog "Your compositor must support this protocol (niri, sway, etc.)."
	elog ""
	elog "Bind a key in your compositor to: psh clipboard"
}
