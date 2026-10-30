# BotGuard DOM bundle

`dom.min.js` is the exact tested RustyPipe Botguard bundle from upstream commit
`f235ebc1ae89d0fa2c103ca705ff1a312b9be673`, decompressed without source changes.
SHA-256: `223dfcdf12fda2662fc8ece93a8ac89d0a02e142e0733581c736f6dfdd90b36e`.
It embeds BgUtils 3.2.0 and jsdom-minimal 26.0.0 plus their bundled dependencies.

Upstream: https://codeberg.org/ThetaDev/rustypipe-botguard

The Rust interpreter executes this bundle in a private worker. No Deno, Node,
browser, npm, JavaScript package manager, native JS engine, process API,
filesystem API or network API is exposed to JavaScript at runtime. Rust performs
bounded HTTP requests separately. Downloaded BotGuard/player scripts are temporary
in-memory inputs; this bundle is the fixed environment, not a cached challenge.

`LICENSE-RUSTYPIPE` preserves the wrapper license. `THIRD_PARTY_NOTICES.txt`
preserves the full notices for the 20 packages listed by upstream's bundled
`js/dependencies.txt`. Their registry archives were checked against their published
integrity metadata; the jsdom fork against the exact upstream pnpm lockfile hash.
Saxes' npm archive omits its license, so its notice is from its published gitHead.
No package installation or build script ran to collect notices.

The native player preparation in `src/web/player.rs` adapts the Unlicense
algorithm from yt-dlp/ejs 0.8.0; its license is retained at
`experiments/botguard/vendor/LICENSE-EJS-UNLICENSE`.
