# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

RUST_MIN_VER="1.85.0"

inherit cargo git-r3

DESCRIPTION="Screen locker for the psh Wayland desktop environment"
HOMEPAGE="https://github.com/idknerdyshit/psh"
EGIT_REPO_URI="https://github.com/idknerdyshit/psh.git"

LICENSE="GPL-3+"
SLOT="0"

DEPEND="
	dev-libs/wayland
	sys-libs/pam
"
RDEPEND="${DEPEND}"

QA_FLAGS_IGNORED="usr/bin/psh-lock"

src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

src_compile() {
	cargo_src_compile --bin psh-lock
}

src_install() {
	dobin "$(cargo_target_dir)/psh-lock"
}

pkg_postinst() {
	elog "psh-lock uses ext-session-lock-v1 — your compositor must support it."
	elog "PAM authentication is used; ensure your PAM config is correct."
	elog ""
	elog "Lock manually: psh lock"
}
