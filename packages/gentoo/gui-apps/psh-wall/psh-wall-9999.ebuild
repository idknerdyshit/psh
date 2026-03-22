# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

RUST_MIN_VER="1.85.0"

inherit cargo git-r3 systemd

DESCRIPTION="Wallpaper manager for the psh Wayland desktop environment"
HOMEPAGE="https://github.com/idknerdyshit/psh"
EGIT_REPO_URI="https://github.com/idknerdyshit/psh.git"

LICENSE="GPL-3+"
SLOT="0"

DEPEND="
	dev-libs/wayland
"
RDEPEND="${DEPEND}"

QA_FLAGS_IGNORED="usr/bin/psh-wall"

src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

src_compile() {
	cargo_src_compile --bin psh-wall
}

src_install() {
	dobin "$(cargo_target_dir)/psh-wall"
	systemd_douserunit "${S}/systemd/psh-wall.service"
}

pkg_postinst() {
	elog "Set a wallpaper in ~/.config/psh/psh.toml:"
	elog "  [wall]"
	elog "  path = \"~/wallpaper.png\""
	elog "  mode = \"fill\"  # fill, fit, center, stretch, tile"
	elog ""
	elog "Change at runtime: psh wall set /path/to/image.png"
}
