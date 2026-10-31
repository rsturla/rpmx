#!/bin/sh
# Prove that recipe-declared Provides/Obsoletes/Conflicts survive the RPM writer
# and drive a real dnf replacement transaction.
#
# Usage: replacement-dnf-test.sh [image]
#
# This uses `rpmx build --stage`, so it needs no build root: the point is the
# package metadata, not compilation.
set -eu

image=${1:-registry.fedoraproject.org/fedora:44}
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
rpmx=${RPMX:-$root/target/release/rpmx}
cache=${XDG_CACHE_HOME:-$HOME/.cache}/rpmx/replacement-test
work=$cache/work
rpms=$cache/rpms
rootfs=$cache/rootfs
repo_old=$cache/repo-old
repo_new=$cache/repo-new

remove_work() {
    if [ -e "$1" ]; then
        podman unshare -- rm -rf "$1" 2>/dev/null ||
            unshare -Ur -- rm -rf "$1" 2>/dev/null ||
            rm -rf "$1" 2>/dev/null || true
    fi
}

remove_work "$cache"
mkdir -p "$work" "$rpms" "$rootfs" "$repo_old" "$repo_new"

make_stage() {
    name=$1
    dir=$2
    mkdir -p "$dir/usr/bin"
    printf '#!/bin/sh\necho %s\n' "$name" >"$dir/usr/bin/$name"
    chmod 0755 "$dir/usr/bin/$name"
}

write_recipe() {
    path=$1
    name=$2
    version=$3
    dependencies=$4
    cat >"$path" <<EOF
schema: 1
package:
  name: $name
  version: "$version"
  release: 1
  summary: replacement fixture $name
  license: MIT
  source-date-epoch: 1704067200
outputs:
  - name: $name
$dependencies
    files:
      include:
        - /usr/bin/$name
EOF
}

make_stage swap-old "$work/old-stage"
make_stage swap-new "$work/new-stage"
make_stage swap-conflict "$work/conflict-stage"

write_recipe "$work/old.yaml" swap-old 1.0 ""
write_recipe "$work/new.yaml" swap-new 2.0 "    dependencies:
      provides:
        - \"swap-old = 2.0-1\"
      obsoletes:
        - \"swap-old < 2.0\""
write_recipe "$work/conflict.yaml" swap-conflict 1.0 "    dependencies:
      conflicts:
        - swap-new"

"$rpmx" build "$work/old.yaml" --stage "$work/old-stage" --output "$rpms" --arch x86_64 >/dev/null
"$rpmx" build "$work/new.yaml" --stage "$work/new-stage" --output "$rpms" --arch x86_64 >/dev/null
"$rpmx" build "$work/conflict.yaml" --stage "$work/conflict-stage" --output "$rpms" --arch x86_64 >/dev/null
cp "$rpms/swap-old-1.0-1.x86_64.rpm" "$repo_old"/
cp "$rpms/swap-new-2.0-1.x86_64.rpm" "$rpms/swap-conflict-1.0-1.x86_64.rpm" "$repo_new"/

echo "metadata:"
rpm -qp --provides "$rpms/swap-new-2.0-1.x86_64.rpm" | grep '^swap-old = 2.0-1$'
rpm -qp --obsoletes "$rpms/swap-new-2.0-1.x86_64.rpm" | grep '^swap-old < 2.0$'
rpm -qp --conflicts "$rpms/swap-conflict-1.0-1.x86_64.rpm" | grep '^swap-new$'

podman run --rm --cap-add=SYS_CHROOT \
    -v "$cache:/work:Z" \
    "$image" sh -ec '
    dnf -q -y install createrepo_c >/dev/null 2>&1
    createrepo_c /work/repo-old >/dev/null
    createrepo_c /work/repo-new >/dev/null
    dnf -q -y --installroot=/work/rootfs \
        --config=/etc/dnf/dnf.conf \
        --setopt=reposdir=/etc/yum.repos.d \
        --releasever=44 --setopt=install_weak_deps=0 \
        install bash coreutils >/dev/null
    mkdir -p /work/empty
    common="--installroot=/work/rootfs --config=/etc/dnf/dnf.conf \
        --setopt=reposdir=/work/empty --releasever=44 --forcearch=x86_64 \
        --setopt=gpgcheck=0 --setopt=repo_gpgcheck=0"

    dnf -q -y $common --repofrompath=old,file:///work/repo-old --repo=old \
        install swap-old >/dev/null
    rpm --root /work/rootfs -q swap-old

    echo "swap:"
    dnf -q -y $common --repofrompath=old,file:///work/repo-old \
        --repofrompath=new,file:///work/repo-new --repo=old,new \
        swap swap-old swap-new >/dev/null
    rpm --root /work/rootfs -q swap-new
    if rpm --root /work/rootfs -q swap-old >/dev/null 2>&1; then
        echo "swap-old is still installed" >&2
        exit 1
    fi
    rpm --root /work/rootfs -q --provides swap-new | grep -qx "swap-old = 2.0-1"
    rpm --root /work/rootfs -q --whatprovides swap-old | grep -qx swap-new-2.0-1.x86_64

    echo "conflict:"
    if dnf -q -y $common --repofrompath=old,file:///work/repo-old \
        --repofrompath=new,file:///work/repo-new --repo=old,new \
        install swap-conflict >/work/conflict.log 2>&1; then
        echo "swap-conflict installed despite Conflicts: swap-new" >&2
        exit 1
    fi
    grep -i "conflict" /work/conflict.log >/dev/null
    echo ok
'

remove_work "$rootfs"
echo "done"
