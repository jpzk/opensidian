# SPDX-License-Identifier: GPL-3.0-or-later
# opensidian — hand-written RPM spec (Fedora only). No tauri bundler, no recompile.
#
# WRAP, DON'T RECOMPILE: Source0 is the release binary the box already built for
# the AppImages (src-tauri/target/release/opensidian). It is installed BYTE-IDENTICAL:
# no strip, no debuginfo split, no build-id rewrite (see the %%global block below).
# The release stage proves it: sha256 of /usr/bin/opensidian out of this rpm
# (rpm2cpio) == sha256 of usr/bin/opensidian inside the slim AppImage, same run.
#
# BUILT IN: the Fedora 44 GA container, pinned by digest (image released 2026-04-22,
# > 6 weeks old; tools come from the frozen GA repo, `dnf --repo=fedora`):
#   Fedora-Container-Base-Generic-44-1.7.x86_64.oci.tar.xz
#     file sha256 75200f5752a74a21a616ca9a75e25beb594e2e117a0195c54f87c0b3e3974d1b
#     (= signed Fedora-Container-44-1.7-x86_64-CHECKSUM)
#   fedora:44@sha256:f1e66cdd6eff2c9ccad192f8865af9be6d69b46b3f13329d2975a2d61a1296c5
#   driver: bin/hz/rpm-build.sh (harness), called by bin/hz/release-build.sh.
#
# ICON: the official logo. packaging/linux/opensidian.svg is installed as the scalable
# hicolor icon; the 48..512 PNGs are packaging/linux/icons/<size>.png, rendered from
# that svg by packaging/linux/render-icons.sh (pinned rsvg-convert) and committed.
# The rpm installs those exact bytes, so every artifact carries the same launcher icon.
#
# REQUIRES: none written by hand. rpmbuild's ELF dependency generator reads the
# binary's DT_NEEDED (libwebkit2gtk-4.1.so.0()(64bit), libgtk-3.so.0, ...).
#
# UNSIGNED: integrity comes from the release's signed SHA256SUMS.

%global debug_package %{nil}
%global __os_install_post %{nil}
%global __brp_strip %{nil}
%global __brp_strip_static_archive %{nil}
%global __brp_strip_comment_note %{nil}
%global _build_id_links none

%global appid dev.koto.opensidian

Name:           opensidian
Version:        %{ver}
Release:        1%{?dist}
Summary:        Local-first markdown vault editor
License:        GPL-3.0-or-later
URL:            https://github.com/jpzk/opensidian
Source0:        opensidian
Source1:        %{appid}.desktop
Source2:        %{appid}.metainfo.xml
Source3:        opensidian.svg
Source4:        LICENSE
Source10:       icon-48.png
Source11:       icon-64.png
Source12:       icon-128.png
Source13:       icon-256.png
Source14:       icon-512.png
ExclusiveArch:  x86_64
BuildRequires:  desktop-file-utils
BuildRequires:  libappstream-glib

%description
opensidian is a local-first markdown editor for a folder of plain files (a
vault): tabbed editing, wiki links, a graph view and custom themes. It keeps
your notes as files on disk and never needs a network connection.

%prep
# nothing to unpack: the sources are the prebuilt release binary and metadata.
cp -p %{SOURCE4} LICENSE

%build
# nothing to build: the binary is prebuilt, the icons are committed renders.

%install
install -Dm0755 %{SOURCE0} %{buildroot}%{_bindir}/opensidian
install -Dm0644 %{SOURCE1} %{buildroot}%{_datadir}/applications/%{appid}.desktop
install -Dm0644 %{SOURCE2} %{buildroot}%{_metainfodir}/%{appid}.metainfo.xml
for s in 48 64 128 256 512; do
  install -Dm0644 %{_sourcedir}/icon-$s.png %{buildroot}%{_datadir}/icons/hicolor/${s}x${s}/apps/%{appid}.png
done
install -Dm0644 %{SOURCE3} %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/%{appid}.svg

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/%{appid}.desktop
appstream-util validate-relax --nonet %{buildroot}%{_metainfodir}/%{appid}.metainfo.xml

%files
%license LICENSE
%{_bindir}/opensidian
%{_datadir}/applications/%{appid}.desktop
%{_metainfodir}/%{appid}.metainfo.xml
%{_datadir}/icons/hicolor/48x48/apps/%{appid}.png
%{_datadir}/icons/hicolor/64x64/apps/%{appid}.png
%{_datadir}/icons/hicolor/128x128/apps/%{appid}.png
%{_datadir}/icons/hicolor/256x256/apps/%{appid}.png
%{_datadir}/icons/hicolor/512x512/apps/%{appid}.png
%{_datadir}/icons/hicolor/scalable/apps/%{appid}.svg

%changelog
* Wed Oct 07 2026 jpzk <jendrik@madewithtea.com> - 0.4-1
- First RPM: the v0.4 release binary, desktop entry, metainfo and hicolor icons (official logo: scalable svg + 48..512 png).
