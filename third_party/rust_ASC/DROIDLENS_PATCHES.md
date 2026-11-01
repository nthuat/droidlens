# droidlens patches to rust_ASC

Vendored from https://github.com/MG1937/rust_ASC at commit
`0daddfb832b24bd0e43e76a9365eab4673db0cec` (Apache-2.0, see `LICENSE`).
droidlens depends on it by path with `default-features = false`, so the
`mcp` feature (rmcp, tokio, serde) is not compiled from this copy.

## Changes

1. `src/asc_core/utils/tinydex.rs`: `DEX` gained `all_strings() -> Rc<Vec<String>>`,
   backed by a `OnceCell`. The string pool is decoded once per DEX instead of
   once per extract.
2. `src/asc_core/core/dex/dex_remapper.rs` and `src/asc_core/utils/dex_parser.rs`:
   `origin_strings` is that shared `Rc<Vec<String>>` instead of a fresh `Vec<String>`
   built by decoding and cloning every string of the DEX on every
   `DexManager::extract_and_rebuild` call.

   Why: unpatched, one extract of a 380-byte class cost 5 to 10 ms because the
   remapper re-decoded the whole string table. Patched and with one `DexManager`
   reused per DEX, an extract costs 0.01 to 0.7 ms and produces byte-identical
   mini DEX output.
3. Removed `.git`, the `tests/` fixtures, `Cargo.lock` files, and the
   `[profile.*]` sections of `Cargo.toml` and `radguard/Cargo.toml` (Cargo ignores
   profiles of non-root packages and warns about them on every build).
4. `src/asc_core/core/dex/dex_remapper.rs` (`handle`): the hollowed string, type,
   field and method sets (`HashSet<u32>`) and the code-item metadata map are
   iterated in sorted order instead of hash order.

   Why: Rust randomizes `HashSet` order per process, so the order in which extra
   strings and types were appended, and therefore the new type indices, changed
   between runs. Annotation sets are sorted by new type index, so the same class
   decompiled with annotations in a different order from one run to the next
   (seen on `kotlin.internal.Exact` and `org.bytedeco.javacpp.annotation.Allocator`).
   After the patch, two full runs over 200 classes from 4 apps are byte-identical.
