[![Rust](https://github.com/madsys-dev/async-spdk/workflows/Rust/badge.svg?branch=main)](https://github.com/madsys-dev/async-spdk/actions)

- use ./configure --without-isal instead of ./configure to avoid isa-unsupported or certain error when congifure SPDK
- run as rooter
    - cargo run --example hello_blob ./examples/hello_blob.json
    - cargo run --example hello_bdev ./examples/hello_bdev.json
- when miss hugepage
    - echo "1024" > /sys/kernel/mm/hugepages/hugepages-2048kB/nr_hugepages

## Store data-node compatibility

The `fix/store-data-node` branch provides the API used by `GatewayJ/store`:

- `BlobStore`, super-blob metadata, xattrs, and independently owned blob iteration.
- Blob reads into `DmaBuf<MaybeUninit<u8>>`, with `assume_init` after successful I/O.
- Byte offsets for asynchronous blob reads, writes, and zeroing, validated before conversion to SPDK I/O units.
- Reactor enumeration, persistent reactor threads, `SpdkPoller`, `spawn_local`, and `SpdkEvent::call_on`.
- `AppOpts::app_start` keeps reactors running after its initialization future completes.
- Negative errno conversion compatible with `nix 0.29` and the data-node protocol.

Build on Linux. Native dependencies include a C/C++ compiler, Clang/libclang,
OpenSSL, libaio, libnuma, libuuid, libelf, Meson, Ninja, pkg-config, and pyelftools.
The build initializes the pinned SPDK and DPDK submodules and runs parallel make
using Cargo's job count. AArch64 uses the generic DPDK platform.
The linked event subsystems initialize block storage services; iSCSI, NVMe-oF,
and vhost target services are excluded from application startup.

```sh
sudo apt-get install build-essential clang libclang-dev libssl-dev libaio-dev \
  libnuma-dev uuid-dev libelf-dev nasm meson ninja-build python3-pyelftools pkg-config
cargo test --lib
SPDK_TEST_CORE_MASK='[0,1]' cargo run --example store_compat
```

Choose cores allowed by the process CPU affinity. The example uses a Malloc
block device and ordinary memory, so it requires no physical NVMe device or
hugepage setup. It checks reactor task dispatch, nonzero byte-offset I/O, xattrs,
super-blob persistence, and blob ownership across enumeration and reload.

To use the existing store manifests, keep `store`, `async-spdk`, and `openraft`
as sibling directories. Use OpenRaft branch `fix/store-meta-node` from
`GatewayJ/openraft`. Building the store workspace also requires `protoc` and
CMake for its protocol and RocksDB dependencies.
