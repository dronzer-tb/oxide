#!/bin/sh
# Root part on s5 (the bench VPS). Run by install.sh as: ssh s5 sh -s "<agents public key>"
# Creates the unprivileged user `bench` with Rust, authorises the oci-v3 agents key for it
# only, and gives it the shared reference data the benchmark needs.
set -eu
id bench >/dev/null 2>&1 || useradd -m -s /bin/bash bench
su - bench -c 'test -x ~/.cargo/bin/cargo || curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null 2>&1'
install -d -o bench -g bench -m 700 /home/bench/.ssh
grep -qF "$1" /home/bench/.ssh/authorized_keys 2>/dev/null || echo "$1" >>/home/bench/.ssh/authorized_keys
chown bench:bench /home/bench/.ssh/authorized_keys
chmod 600 /home/bench/.ssh/authorized_keys
install -d -o bench -g bench /home/bench/oxide-perf
if [ ! -d /home/bench/oxide-perf/reference ]; then
    cp -r /root/oxide-perf/base/reference /home/bench/oxide-perf/reference
    chown -R bench:bench /home/bench/oxide-perf/reference
fi
su - bench -c '~/.cargo/bin/cargo --version'
