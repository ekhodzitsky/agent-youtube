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

## Public client API key

The API key used by `src/web/runtime.rs`, the research adapter and this bundle is
publicly distributed in YouTube client JavaScript. On 2026-10-30 its exact value
matched the `X-Goog-Api-Key` construction in the
[current player script](https://www.youtube.com/s/player/11a45ffb/player_ias.vflset/en_US/base.js)
referenced by a freshly fetched watch page. The script SHA-256 was
`c07fdf714a0dad0bcb0c25c4bfc8ac7ec61c8fd8aeef72284f838d107dbe2ef0`.
It also matches the integrity-token request in a
[historical YouTube bundle](https://www.youtube.com/s/desktop/d5c4364e/jsbin/desktop_polymer.vflset/desktop_polymer.js)
and [BgUtils constants from 2024](https://github.com/LuanRT/BgUtils/blob/7f7c1d2c97b800fa9234b94ca263c2937e962bd6/src/utils/constants.ts).

The Rust runtime sends it to YouTube's `/api/jnn/v1/GenerateIT` endpoint with a
BotGuard snapshot, then uses the response to mint video-bound PO tokens. It is
an API key, not merely an arbitrary protocol identifier or the generated PO token.
This provenance explains a secret-scanning match; it does not establish the
key's Cloud IAM permissions, API restrictions, billing configuration or future
acceptance. Those settings were not inspected. Public distribution does not grant
permission to use unrelated Google APIs. No claim of revocation or universal
harmlessness is made, and the bundle remains byte-for-byte unchanged.
