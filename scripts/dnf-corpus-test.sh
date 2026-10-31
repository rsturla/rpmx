#!/bin/sh
# Exercise a set of rpmx-built RPMs through a real dnf transaction:
#   local repo -> clean installroot -> install -> smoke -> upgrade -> remove.
#
# Usage: dnf-corpus-test.sh <rpm-dir> <upgrade-dir> [image] [public-key]
#
# With a public key the repo is verified with gpgcheck=1 and the key is imported
# into the installroot. Without one the packages are treated as unsigned and the
# local repo disables gpgcheck.
#
# zlib is intentionally not part of this set: Fedora's zlib-ng-compat owns the
# zlib virtual name and files, and that replacement policy belongs in a profile,
# not in a generic recipe.
set -eu

rpms=${1:?usage: dnf-corpus-test.sh RPM_DIR UPGRADE_DIR [IMAGE] [PUBLIC_KEY]}
upgrade=${2:?usage: dnf-corpus-test.sh RPM_DIR UPGRADE_DIR [IMAGE] [PUBLIC_KEY]}
image=${3:-registry.fedoraproject.org/fedora:44}
public_key=${4:-}

case "$rpms" in /*) ;; *) rpms=$(cd "$rpms" && pwd) ;; esac
case "$upgrade" in /*) ;; *) upgrade=$(cd "$upgrade" && pwd) ;; esac
if [ -n "$public_key" ]; then
    case "$public_key" in /*) ;; *) public_key=$(cd "$(dirname "$public_key")" && pwd)/$(basename "$public_key") ;; esac
    gpgcheck=1
else
    gpgcheck=0
fi

cache=${XDG_CACHE_HOME:-$HOME/.cache}/rpmx/dnf-test
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
rpmx=${RPMX:-$root/target/release/rpmx}
rootfs=$cache/rootfs
repo=$cache/repo
repo2=$cache/repo2
work=$cache/work

remove_work() {
    if [ -e "$1" ]; then
        podman unshare -- rm -rf "$1" 2>/dev/null ||
            unshare -Ur -- rm -rf "$1" 2>/dev/null ||
            rm -rf "$1" 2>/dev/null || true
    fi
}

remove_work "$cache"
mkdir -p "$repo" "$repo2" "$rootfs" "$work"
cp "$rpms"/*.rpm "$repo"/
cp "$upgrade"/*.rpm "$repo2"/
if [ -n "$public_key" ]; then
    cp "$public_key" "$cache/signing-key.asc"
fi
"$rpmx" repo --rpms "$repo" --output "$repo" >/dev/null
"$rpmx" repo --rpms "$repo2" --output "$repo2" >/dev/null

podman run --rm --cap-add=SYS_CHROOT \
    -e GPGCHECK="$gpgcheck" \
    -v "$cache:/work:Z" \
    "$image" sh -ec '

    mkdir -p /work/empty
    dnf -q -y --installroot=/work/rootfs \
        --config=/etc/dnf/dnf.conf \
        --setopt=reposdir=/etc/yum.repos.d \
        --releasever=44 --setopt=install_weak_deps=0 \
        install bash coreutils python3 oniguruma >/dev/null

    dbpath="/work/rootfs$(rpm --root /work/rootfs --eval %{_dbpath})"
    if [ "$GPGCHECK" = 1 ]; then
        rpm --dbpath "$dbpath" --import /work/signing-key.asc
    fi

    common="--installroot=/work/rootfs --config=/etc/dnf/dnf.conf \
        --setopt=reposdir=/work/empty --releasever=44 --forcearch=x86_64 \
        --setopt=gpgcheck=$GPGCHECK --setopt=repo_gpgcheck=0"

    dnf -y $common --repofrompath=rpmx,file:///work/repo --repo=rpmx \
        install lua pyhello c-hello jq libjq >/dev/null

    echo "installed:"
    rpm --root /work/rootfs -q lua pyhello c-hello jq libjq

    echo "smoke:"
    chroot /work/rootfs /bin/sh -c "lua -e \"assert(1+1==2)\" && \
        python3 -c \"import pyhello; assert pyhello.answer()==42\" && \
        echo \"{\\\"a\\\":1}\" | jq -r .a && \
        c-hello && test -f /usr/share/c-hello.marker"

    echo "upgrade:"
    dnf -y $common --repofrompath=rpmx2,file:///work/repo2 --repo=rpmx2 \
        upgrade pyhello >/dev/null
    rpm --root /work/rootfs -q --qf "%{NAME}-%{VERSION}-%{RELEASE}\n" pyhello

    if [ "$GPGCHECK" = 1 ]; then
        echo "signatures:"
        rpm --dbpath "$dbpath" --checksig /work/repo/*.rpm /work/repo2/*.rpm
    fi

    echo "remove:"
    dnf -y $common remove lua pyhello c-hello jq libjq >/dev/null
    for path in /usr/bin/lua /usr/bin/jq /usr/bin/c-hello; do
        test ! -e "/work/rootfs$path" || { echo "still present: $path"; exit 1; }
    done
    echo ok
'

remove_work "$rootfs"
remove_work "$work"
