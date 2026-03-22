# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

RUST_MIN_VER="1.85.0"

inherit cargo git-r3 systemd

DESCRIPTION="Idle monitor daemon for the psh Wayland desktop environment"
HOMEPAGE="https://github.com/idknerdyshit/psh"
EGIT_REPO_URI="https://github.com/idknerdyshit/psh.git"

LICENSE="GPL-3+"
SLOT="0"

DEPEND="
	dev-libs/wayland
	sys-auth/polkit
"
RDEPEND="${DEPEND}
	gui-apps/psh-lock
"

QA_FLAGS_IGNORED="usr/bin/psh-idle"

src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

src_compile() {
	cargo_src_compile --bin psh-idle
}

src_install() {
	dobin "$(cargo_target_dir)/psh-idle"
	systemd_douserunit "${S}/systemd/psh-idle.service"
}

pkg_postinst() {
	elog "psh-idle spawns psh-lock on idle timeout and before system sleep."
	elog ""
	elog "Configure in ~/.config/psh/psh.toml:"
	elog "  [idle]"
	elog "  idle_timeout_secs = 300  # 0 to disable"
	elog "  lock_on_sleep = true"
}
