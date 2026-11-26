#!/bin/bash
# Copy this repo and sbm-common into the builder VM and start the image build.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
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
    "${root}/../sbm-common/" builder@127.0.0.1:/home/builder/src/sbm-common/

"${ssh[@]}" builder@127.0.0.1 'chmod +x /home/builder/src/sbm-core/scripts/build-inside.sh && cd /home/builder/src/sbm-core && nohup /home/builder/src/sbm-core/scripts/build-inside.sh > /home/builder/image-build.log 2>&1 & echo STARTED'
