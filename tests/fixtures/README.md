# Codec regression fixture

`ps48.m4a` is the generated stereo sine HE-AACv2 sample from syom commit
`997bf56efda8700792dbc4b0136a36ad90eca328`, `src/goldens/ps48.m4a`.
SHA-256: `74e3e7fa97363b7fb70aae783da15d0623f9818f44458926f2b98cdab98f35cf`.
It is covered by the upstream MIT license retained at vendor/syom/LICENSE.
The upstream generation recipe uses 440 Hz and 880 Hz sine channels and Apple's
AAC encoder; this is synthetic audio, not a recording of a person.

The file has 26 coded AAC frames. After priming, the independently decoded
FFmpeg golden contains 204544 bytes of stereo PCM16: 51136 samples per channel.
Its movie edit duration is 1.066 s at 48 kHz (51168 samples), a 32-sample upward
rounding difference. The regression accepts that bounded metadata discrepancy
without synthesizing samples, and rejects a further one-millisecond extension.
