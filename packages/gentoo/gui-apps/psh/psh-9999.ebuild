# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

inherit cargo git-r3

DESCRIPTION="psh Wayland desktop environment (meta-package)"
HOMEPAGE="https://github.com/idknerdyshit/psh"
EGIT_REPO_URI="https://github.com/idknerdyshit/psh.git"

LICENSE="GPL-3+"
SLOT="0"

RDEPEND="
	gui-apps/psh-bar
	gui-apps/psh-notify
	gui-apps/psh-polkit
	gui-apps/psh-launch
	gui-apps/psh-clip
	gui-apps/psh-wall
	gui-apps/psh-lock
	gui-apps/psh-idle
"
PDEPEND="virtual/notification-daemon"

QA_FLAGS_IGNORED="usr/bin/psh"

src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

src_compile() {
	cargo_src_compile --bin psh
}

src_install() {
	# Install the psh CLI control binary
	dobin "$(cargo_target_dir)/psh"

	# Install the systemd target that ties all components together
	insinto /usr/lib/systemd/user
	doins "${S}/systemd/psh.target"

	# Install shared assets (themes)
	insinto /usr/share/psh/themes
	doins "${S}/assets/themes/"*

	# Install example configs
	insinto /usr/share/doc/${PF}
	doins "${S}/config/psh.toml"
	doins "${S}/config/niri.kdl"
}

pkg_postinst() {
	elog "To start the psh desktop environment:"
	elog "  systemctl --user enable --now psh.target"
	elog ""
	elog "Example configs installed to /usr/share/doc/${PF}/:"
	elog "  psh.toml  — psh component configuration"
	elog "  niri.kdl  — example niri compositor config with psh keybindings"
	elog ""
	elog "Copy to get started:"
	elog "  mkdir -p ~/.config/psh"
	elog "  cp /usr/share/doc/${PF}/psh.toml ~/.config/psh/"
	elog ""
	elog "CLI control tool: psh --help"
}
