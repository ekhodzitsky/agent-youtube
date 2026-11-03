# Supplemental upstream license texts

The three hashed verbatim texts complete dependency notices for crate archives that
omit their license files. They are attribution records, not runtime code.

Exact package versions, immutable upstream commit URLs and SHA-256 digests are
in [the source map](../../tools/license-sources.json). Several Boa packages share
the same two upstream texts. The existing local Boa copies use the texts already
retained in `experiments/botguard/vendor/`. r-efi carries its grants in AUTHORS.

`syom-MIT.txt` separately preserves the original MIT grant for the retained
AAC test fixture after removal of the syom source fork. The active Git dependency
also provides its own LICENSE for the collector.

Run [the collector](../../tools/prepare_notices.py) to assemble the resolved
locked graph and bundled JavaScript notices. It verifies supplemental file
hashes and refuses altered copies. The generated collection is local-only until
distribution policy and the project license grant are settled; collecting text
does not select license alternatives or approve redistribution.
