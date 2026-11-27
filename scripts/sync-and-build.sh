#!/bin/bash
# Copy this repo and sbm-common into the builder VM and start the image build.
# Forwarded environment variables:
#   SBM_BUILD_PROFILE, SBM_ARCHES, SBM_INCREMENTAL, SBM_COMPRESS,
#   SBM_FORCE_COMPRESS, SBM_ENABLE_SMOKE, CARGO_BUILD_JOBS
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
common_dir="${root}/../sbm-common"
expected_common_rev=${SBM_COMMON_REV:-abe3029c831f05d77d10cfb214a0c236903d00d9}

if [ ! -d "${common_dir}/.git" ]; then
    echo "expected sbm-common git checkout at ${common_dir}" >&2
    exit 1
fi
actual_common_rev=$(git -C "$common_dir" rev-parse HEAD)
if [ "$actual_common_rev" != "$expected_common_rev" ]; then
    echo "sbm-common HEAD mismatch: got ${actual_common_rev}, expected ${expected_common_rev}" >&2
    echo "checkout ${expected_common_rev} in ${common_dir} or override SBM_COMMON_REV" >&2
    exit 1
fi
if [ "${SBM_BUILD_PROFILE:-dev}" = "release" ]; then
    if [ -n "$(git -C "$root" status --porcelain)" ]; then
        echo "refusing release build from dirty sbm-core working tree" >&2
        exit 1
    fi
    if [ -n "$(git -C "$common_dir" status --porcelain)" ]; then
        echo "refusing release build from dirty sbm-common working tree" >&2
        exit 1
    fi
fi

"${root}/scripts/start-builder.sh" start
key="${root}/build/builder/id_ed25519"
known="${root}/build/builder/known_hosts"
ssh=(ssh -i "$key" -p 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile="$known")
rsync=(rsync -a --delete -e "ssh -i ${key} -p 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile=${known}")

"${ssh[@]}" builder@127.0.0.1 'mkdir -p /home/builder/src'
"${rsync[@]}" \
    --exclude build --exclude target --exclude .git \
    --exclude mkosi.key --exclude mkosi.crt \
    "${root}/" builder@127.0.0.1:/home/builder/src/sbm-core/
"${rsync[@]}" \
    --exclude target --exclude .git \
    "${common_dir}/" builder@127.0.0.1:/home/builder/src/sbm-common/

remote_env=""
for var in SBM_BUILD_PROFILE SBM_ARCHES SBM_INCREMENTAL SBM_COMPRESS SBM_FORCE_COMPRESS SBM_ENABLE_SMOKE CARGO_BUILD_JOBS; do
    if [ -n "${!var:-}" ]; then
        remote_env="${remote_env} ${var}=$(printf '%q' "${!var}")"
    fi
done

"${ssh[@]}" builder@127.0.0.1 \
    "chmod +x /home/builder/src/sbm-core/scripts/build-inside.sh && cd /home/builder/src/sbm-core && setsid env${remote_env} /home/builder/src/sbm-core/scripts/build-inside.sh > /home/builder/image-build.log 2>&1 < /dev/null & echo STARTED \$!"
