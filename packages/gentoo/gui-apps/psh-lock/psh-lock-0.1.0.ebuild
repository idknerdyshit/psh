# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

CRATES=""

inherit cargo

DESCRIPTION="Screen locker for the psh Wayland desktop environment"
HOMEPAGE="https://github.com/idknerdyshit/psh"
SRC_URI="https://github.com/idknerdyshit/psh/archive/v${PV}.tar.gz -> psh-${PV}.tar.gz"

S="${WORKDIR}/psh-${PV}"

LICENSE="GPL-3+"
SLOT="0"
KEYWORDS="~amd64"

DEPEND="
	dev-libs/wayland
	sys-libs/pam
"
RDEPEND="${DEPEND}"
BDEPEND="virtual/rust"

QA_FLAGS_IGNORED="usr/bin/psh-lock"

src_compile() {
	cargo_src_compile --bin psh-lock
}

src_install() {
	dobin "$(cargo_target_dir)/psh-lock"
}
