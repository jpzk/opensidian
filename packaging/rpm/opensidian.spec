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
# ICON: every hicolor size is generated at build time from ONE file,
# packaging/linux/icon-source.png. Swapping the logo is a one-file change.
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
Source3:        icon-source.png
Source4:        LICENSE
ExclusiveArch:  x86_64
BuildRequires:  ImageMagick
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
# Deterministic PNGs: -strip and no date/time chunks, so the same source always
# yields the same bytes and a different source always yields different ones.
set -- $(magick identify -format '%%w %%h' %{SOURCE3}); W=$1; H=$2
echo "icon-source.png: ${W}x${H}"
if [ "$W" -lt 512 ] || [ "$H" -lt 512 ]; then
  echo "NOTE: icon-source.png is ${W}x${H} (< 512 px): larger sizes are UPSCALED"
fi
for s in 48 64 128 256 512; do
  magick %{SOURCE3} -resize ${s}x${s} -background none -gravity center -extent ${s}x${s} \
    -strip -define png:exclude-chunks=date,time icon-$s.png
  echo "icon $s: $(magick identify -format '%%wx%%h' icon-$s.png) $(sha256sum icon-$s.png | cut -c1-16)"
done

%install
install -Dm0755 %{SOURCE0} %{buildroot}%{_bindir}/opensidian
install -Dm0644 %{SOURCE1} %{buildroot}%{_datadir}/applications/%{appid}.desktop
install -Dm0644 %{SOURCE2} %{buildroot}%{_metainfodir}/%{appid}.metainfo.xml
for s in 48 64 128 256 512; do
  install -Dm0644 icon-$s.png %{buildroot}%{_datadir}/icons/hicolor/${s}x${s}/apps/%{appid}.png
done

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

%changelog
* Wed Oct 07 2026 jpzk <jendrik@madewithtea.com> - 0.4-1
- First RPM: the v0.4 release binary, desktop entry, metainfo and hicolor icons.
