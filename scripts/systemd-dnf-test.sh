#!/bin/sh
# Exercise a systemd service package through dnf: install, scriptlets, sysusers,
# tmpfiles, config(noreplace) upgrade and erase.
#
# Usage: systemd-dnf-test.sh <v1-rpm-dir> <v2-rpm-dir> [image]
set -eu

v1=${1:?usage: systemd-dnf-test.sh V1_DIR V2_DIR [IMAGE]}
v2=${2:?usage: systemd-dnf-test.sh V1_DIR V2_DIR [IMAGE]}
image=${3:-registry.fedoraproject.org/fedora:44}

case "$v1" in /*) ;; *) v1=$(cd "$v1" && pwd) ;; esac
case "$v2" in /*) ;; *) v2=$(cd "$v2" && pwd) ;; esac

cache=${XDG_CACHE_HOME:-$HOME/.cache}/rpmx/systemd-test
rootfs=$cache/rootfs
repo1=$cache/repo1
repo2=$cache/repo2

remove_work() {
    if [ -e "$1" ]; then
        podman unshare -- rm -rf "$1" 2>/dev/null ||
            unshare -Ur -- rm -rf "$1" 2>/dev/null ||
            rm -rf "$1" 2>/dev/null || true
    fi
}

remove_work "$cache"
mkdir -p "$rootfs" "$repo1" "$repo2"
cp "$v1"/*.rpm "$repo1"/
cp "$v2"/*.rpm "$repo2"/

podman run --rm --cap-add=SYS_CHROOT \
    -v "$cache:/work:Z" \
    "$image" sh -ec '
    dnf -q -y install createrepo_c systemd-analyze >/dev/null 2>&1
    createrepo_c /work/repo1 >/dev/null
    createrepo_c /work/repo2 >/dev/null
    dnf -q -y --installroot=/work/rootfs \
        --config=/etc/dnf/dnf.conf \
        --setopt=reposdir=/etc/yum.repos.d \
        --releasever=44 --setopt=install_weak_deps=0 \
        install bash coreutils systemd >/dev/null

    # Record scriptlet calls instead of talking to a systemd that is not PID 1.
    cat > /work/rootfs/usr/bin/systemctl <<"EOF"
#!/bin/sh
echo "$*" >> /var/log/systemctl.log
EOF
    chmod 0755 /work/rootfs/usr/bin/systemctl

    mkdir -p /work/empty
    common="--installroot=/work/rootfs --config=/etc/dnf/dnf.conf \
        --setopt=reposdir=/work/empty --releasever=44 --forcearch=x86_64 \
        --setopt=gpgcheck=0 --setopt=repo_gpgcheck=0"
    both="--repofrompath=r1,file:///work/repo1 \
        --repofrompath=r2,file:///work/repo2 --repo=r1,r2"

    echo "install:"
    dnf -q -y $common --repofrompath=r1,file:///work/repo1 --repo=r1 \
        install hello-daemon >/dev/null
    rpm --root /work/rootfs -q hello-daemon
    test -f /work/rootfs/usr/lib/systemd/system/hello-daemon.service
    test -f /work/rootfs/usr/lib/tmpfiles.d/hello-daemon.conf
    grep -q "^hello-daemon:" /work/rootfs/etc/passwd
    systemd-analyze verify --root=/work/rootfs hello-daemon.service
    chroot /work/rootfs /usr/bin/hello-daemon --version

    grep -q -- "--no-reload preset hello-daemon.service" /work/rootfs/var/log/systemctl.log
    if grep -q "disable --now" /work/rootfs/var/log/systemctl.log; then
        echo "install disabled the service" >&2
        exit 1
    fi

    echo "upgrade:"
    printf "greeting=local\n" > /work/rootfs/etc/hello-daemon.conf
    dnf -q -y $common $both upgrade hello-daemon >/dev/null
    rpm --root /work/rootfs -q --qf "%{NAME}-%{VERSION}-%{RELEASE}\n" hello-daemon
    grep -qx "greeting=local" /work/rootfs/etc/hello-daemon.conf
    grep -qx "greeting=hello-v2" /work/rootfs/etc/hello-daemon.conf.rpmnew
    if grep -q "disable --now" /work/rootfs/var/log/systemctl.log; then
        echo "upgrade disabled the service" >&2
        exit 1
    fi

    echo "erase:"
    dnf -q -y $common $both remove hello-daemon >/dev/null
    test ! -e /work/rootfs/usr/bin/hello-daemon
    test ! -e /work/rootfs/usr/lib/systemd/system/hello-daemon.service
    grep -qx "greeting=local" /work/rootfs/etc/hello-daemon.conf.rpmsave
    grep -q -- "--no-reload disable --now hello-daemon.service" /work/rootfs/var/log/systemctl.log
    echo ok
'

remove_work "$rootfs"
echo "done"
