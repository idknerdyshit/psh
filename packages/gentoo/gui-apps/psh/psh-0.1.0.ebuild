# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

DESCRIPTION="psh Wayland desktop environment (meta-package)"
HOMEPAGE="https://github.com/psh-de/psh"
SRC_URI="https://github.com/psh-de/psh/archive/v${PV}.tar.gz -> psh-${PV}.tar.gz"

S="${WORKDIR}/psh-${PV}"

LICENSE="GPL-3+"
SLOT="0"
KEYWORDS="~amd64"

RDEPEND="
	=gui-apps/psh-bar-${PV}
	=gui-apps/psh-notify-${PV}
	=gui-apps/psh-polkit-${PV}
	=gui-apps/psh-launch-${PV}
	=gui-apps/psh-clip-${PV}
	=gui-apps/psh-wall-${PV}
	=gui-apps/psh-lock-${PV}
"

src_install() {
	# Install the systemd target that ties all components together
	insinto /usr/lib/systemd/user
	doins "${S}/systemd/psh.target"

	# Install shared assets (themes)
	insinto /usr/share/psh/themes
	doins "${S}/assets/themes/"*

	# Install example config
	insinto /usr/share/doc/${PF}
	doins "${S}/config/psh.toml" 2>/dev/null || true
}
