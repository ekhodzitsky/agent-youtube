# Local Boa corrections

These private corrected copies are the crates.io source archives for Boa
0.22.0, upstream commit `337a3668a0dc86dd401ea20906e782249a64a228`:

| Archive | SHA-256 |
| --- | --- |
| boa_engine-0.22.0.crate | 5d6aaf7fdc4299df68adb67e1a5440a51d34630a1f5271b9698fd74bc97b60d9 |
| boa_ast-0.22.0.crate | 9221b0a090572b4a118b46c5874c0781fe4bc9546b85f0a615b8bfb609ca4ed7 |

The original crate archives omit license files. LICENSE-MIT and
LICENSE-UNLICENSE here are from that exact upstream commit. Original source,
credits and license declarations are preserved. This repository and all its
packages remain private and must not be published.

`changes.patch` is a review aid relative to those archives. It contains:

- A function code block freezes its original function-scope binding count.
  Each runtime invocation marks later eval-added slots absent and deletable.
  Only declarations in that invocation activate those slots; lexical TDZ and
  ordinary parameters/variables keep their existing initialization behavior.
- Missing runtime slots are absent, including when another invocation grows the
  shared compile-time schema. Outer lookup and `with` receiver lookup skip absent
  slots rather than treating an uninitialized value as a missing lexical binding.
- Scope analysis marks implicit `arguments` accessed when direct eval can read
  it. Arrow functions resolve the enclosing arguments object. This also retains
  mapped-argument aliasing when eval accesses arguments indirectly.
- Both manifests forbid publication and omit their ignored package resolver
  setting inside the experiment workspace. The engine's unused jemallocator
  development dependency is omitted: it is not referenced by library tests and
  otherwise requires a native C build. No production allocator is changed.
- One upstream documentation word is quoted for current Clippy compatibility.

The unsafe name-specific `diagnostic-eval.patch` outside this directory is not
applied. Browser Annex B semantics are explicitly enabled by the probe manifest;
this feature adds no package dependency. Keeping source here makes the corrected
engine reproducible without modifying Cargo's cache or another project. The
copies total about 7.8 MiB. Both the main application and isolated probe use this fork.

Run from the repository root:

```sh
cargo test --locked --manifest-path experiments/botguard/Cargo.toml
cargo test --locked --manifest-path experiments/botguard/Cargo.toml -p boa_engine --no-default-features --features annex-b --lib
cargo test --locked --manifest-path experiments/botguard/Cargo.toml -p boa_ast --lib
```

The isolated probe defaults to itself as the workspace member. Upstream development
packages are in this isolated workspace lockfile for repeatable engine tests;
they are not normal/build runtime dependencies of the probe or main application.
