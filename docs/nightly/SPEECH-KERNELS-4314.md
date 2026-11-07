# Speech preparation kernels

Reference: T3 Code nightly4314, `83a3674ffd84ffb33de768210c806cb1ac53fd80`.

The owned PCM decoder copies only the supplied byte view, rejects empty,
misaligned or over-five-minute input, and rejects non-finite or out-of-range
samples. Validated PCM produces the native 16 kHz mono signed 16-bit WAV
representation. Conversion uses JavaScript rounding at negative half boundaries.
No microphone, engine, network request or model file is involved.

Language selection preserves the loaded GGUF spelling, including regional tags;
the effective model language follows the exact native preference order.
Vocabulary resolution keeps authored spellings and alias order, gives project
entries precedence over personal ones, places new hints first, applies
JavaScript whitespace semantics and limits the result to 100 entries. The input
dictionary must already have passed the actual options schema. Unicode folding
uses the compiled Rust Unicode tables. Full Node Unicode 17 locale behavior and
raw lone-surrogate strings remain qualified.

The reproducible private oracle executes whole pinned functions with one
canonical Effect 4.1.2 instance and verifies the complete source snapshot manifest.
It records 20 PCM boundaries, four WAV vectors, 66 loaded-language choices,
20 effective-language choices and 27 vocabulary projections. The maximum-size
PCM case uses synthetic raw bytes only. Fixtures are retained without tree
deletion. These preparation APIs do not activate audio HTTP routes, downloads,
a worker, a transcription engine or voice capabilities.
