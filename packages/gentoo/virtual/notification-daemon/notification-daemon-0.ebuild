# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

DESCRIPTION="Virtual for notification daemon dbus service"
SLOT="0"
KEYWORDS="~amd64"

RDEPEND="
	|| (
		gui-apps/psh-notify
		x11-misc/notification-daemon
		gnome-base/gnome-shell
		gnome-extra/cinnamon
		gui-apps/mako
		xfce-extra/xfce4-notifyd
		x11-misc/notify-osd
		x11-misc/dunst
		>=x11-wm/awesome-3.4.4[dbus]
		x11-wm/enlightenment
		x11-misc/mate-notification-daemon
		lxqt-base/lxqt-notificationd
		net-misc/eventd[notification]
	)
"
